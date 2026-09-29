//! Generate a CallGraph for instances in each crate.
//! You can roughly think of instances as a monomorphic function.
//! If an instance calls another instance, then we have an edge
//! from caller to callee with callsite locations as edge weight.
//! This is a fundamental analysis for other analysis,
//! e.g., points-to analysis, lockguard collector, etc.
//! We also track where a closure is defined rather than called
//! to record the defined function and the parameter of the closure,
//! which is pointed to by upvars.
extern crate rustc_data_structures;
extern crate rustc_hir;

use petgraph::algo;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::visit::IntoNodeReferences;
use petgraph::Direction::Incoming;
use petgraph::{Directed, Graph};

use rustc_data_structures::fx::FxHashMap;
use rustc_hir::def_id::DefId;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    Body, Local, LocalDecl, LocalKind, Location, Operand, PlaceRef, Terminator, TerminatorKind,
};
use rustc_middle::ty::{self, EarlyBinder, Instance, InstanceKind, TyCtxt, TyKind, TypingEnv};

use crate::analysis::pointsto::{Andersen, ConstraintNode, PointsToMap};

/// The NodeIndex in CallGraph, denoting a unique instance in CallGraph.
pub type InstanceId = NodeIndex;

/// The location where caller calls callee.
/// Support direct call for now, where callee resolves to FnDef.
/// Also support tracking the parameter of a closure (pointed to by upvars)
/// TODO(boqin): Add support for FnPtr.
#[derive(Copy, Clone, Debug)]
pub enum CallSiteLocation {
    Direct(Location),
    ClosureDef(Local, Option<Location>),
    // Indirect(Location),
}

impl CallSiteLocation {
    pub fn location(&self) -> Option<Location> {
        match self {
            Self::Direct(loc) | Self::ClosureDef(_, Some(loc)) => Some(*loc),
            _ => None,
        }
    }
}

/// The CallGraph node wrapping an Instance.
/// WithBody means the Instance owns body.
#[derive(Debug, PartialEq, Eq)]
pub enum CallGraphNode<'tcx> {
    WithBody(Instance<'tcx>),
    WithoutBody(Instance<'tcx>),
}

impl<'tcx> CallGraphNode<'tcx> {
    pub fn instance(&self) -> &Instance<'tcx> {
        match self {
            CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) => inst,
        }
    }

    pub fn match_instance(&self, other: &Instance<'tcx>) -> bool {
        matches!(self, CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) if inst == other)
    }
}

/// CallGraph
/// The nodes of CallGraph are instances.
/// The directed edges are CallSite Locations.
/// e.g., `Instance1--|[CallSite1, CallSite2]|-->Instance2`
/// denotes `Instance1` calls `Instance2` at locations `Callsite1` and `CallSite2`.
pub struct CallGraph<'tcx> {
    pub graph: Graph<CallGraphNode<'tcx>, Vec<CallSiteLocation>, Directed>,
}

impl<'tcx> CallGraph<'tcx> {
    /// Create an empty CallGraph.
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
        }
    }

    /// Search for the InstanceId of a given instance in CallGraph.
    pub fn instance_to_index(&self, instance: &Instance<'tcx>) -> Option<InstanceId> {
        self.graph
            .node_references()
            .find(|(_idx, inst)| inst.match_instance(instance))
            .map(|(idx, _)| idx)
    }

    /// Get the instance by InstanceId.
    pub fn index_to_instance(&self, idx: InstanceId) -> Option<&CallGraphNode<'tcx>> {
        self.graph.node_weight(idx)
    }

    /// Perform callgraph analysis on the given instances.
    /// The instances should be **all** the instances with MIR available in the current crate.
    pub fn analyze(
        &mut self,
        instances: Vec<Instance<'tcx>>,
        tcx: TyCtxt<'tcx>,
        typing_env: TypingEnv<'tcx>,
    ) {
        let idx_insts = instances
            .into_iter()
            .map(|inst| {
                let idx = self.graph.add_node(CallGraphNode::WithBody(inst));
                (idx, inst)
            })
            .collect::<Vec<_>>();
        let mut arg_uses = ArgUses::new(tcx, typing_env);
        for (caller_idx, caller) in idx_insts {
            let body = tcx.instance_mir(caller.def);
            // Skip promoted src
            if body.source.promoted.is_some() {
                continue;
            }
            let mut collector =
                CallSiteCollector::new(caller, body, tcx, typing_env, &mut arg_uses);
            collector.visit_body(body);
            for (callee, location) in collector.finish() {
                let callee_idx = if let Some(callee_idx) = self.instance_to_index(&callee) {
                    callee_idx
                } else {
                    self.graph.add_node(CallGraphNode::WithoutBody(callee))
                };
                if let Some(edge_idx) = self.graph.find_edge(caller_idx, callee_idx) {
                    // Update edge weight.
                    self.graph.edge_weight_mut(edge_idx).unwrap().push(location);
                } else {
                    // Add edge if not exists.
                    self.graph.add_edge(caller_idx, callee_idx, vec![location]);
                }
            }
        }
    }

    /// Find the callsites (weight) on the edge from source to target.
    pub fn callsites(
        &self,
        source: InstanceId,
        target: InstanceId,
    ) -> Option<Vec<CallSiteLocation>> {
        let edge = self.graph.find_edge(source, target)?;
        self.graph.edge_weight(edge).cloned()
    }

    /// Find all the callers that call target
    pub fn callers(&self, target: InstanceId) -> Vec<InstanceId> {
        self.graph.neighbors_directed(target, Incoming).collect()
    }

    /// Find all simple paths from source to target.
    /// e.g., for one of the paths, `source --> instance1 --> instance2 --> target`,
    /// the return is [source, instance1, instance2, target].
    pub fn all_simple_paths(&self, source: InstanceId, target: InstanceId) -> Vec<Vec<InstanceId>> {
        algo::all_simple_paths::<Vec<_>, _>(&self.graph, source, target, 0, None)
            .collect::<Vec<_>>()
    }

    /// Print the callgraph in dot format.
    #[allow(dead_code)]
    pub fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}

/// Visit Terminator and record callsites (callee + location).
struct CallSiteCollector<'a, 'tcx> {
    caller: Instance<'tcx>,
    body: &'a Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    typing_env: TypingEnv<'tcx>,
    callsites: Vec<(Instance<'tcx>, CallSiteLocation)>,
    arg_uses: &'a mut ArgUses<'tcx>,
}

impl<'a, 'tcx> CallSiteCollector<'a, 'tcx> {
    fn new(
        caller: Instance<'tcx>,
        body: &'a Body<'tcx>,
        tcx: TyCtxt<'tcx>,
        typing_env: TypingEnv<'tcx>,
        arg_uses: &'a mut ArgUses<'tcx>,
    ) -> Self {
        Self {
            caller,
            body,
            tcx,
            typing_env,
            callsites: Vec::new(),
            arg_uses,
        }
    }

    /// The locations where the closure in `closure` may run: the calls running it or a value
    /// holding it (a `Box`, a reference, a struct), as found by [`ArgUses`].
    fn closure_run_locations(&mut self, closure: Local) -> Vec<Location> {
        self.arg_uses
            .uses(self.caller, closure)
            .into_iter()
            .filter(|(_, call_use)| *call_use == CallUse::Run)
            .map(|(location, _)| location)
            .collect()
    }
    /// Consumes `CallSiteCollector` and returns its callsites when finished visiting.
    fn finish(self) -> impl IntoIterator<Item = (Instance<'tcx>, CallSiteLocation)> {
        self.callsites.into_iter()
    }
}

impl<'tcx> Visitor<'tcx> for CallSiteCollector<'_, 'tcx> {
    /// Resolve direct call.
    /// Inspired by rustc_mir/src/transform/inline.rs#get_valid_function_call.
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Call { ref func, .. } = terminator.kind {
            if let Some((_, Some(callee))) =
                resolve_call(self.tcx, self.typing_env, self.caller, self.body, func)
            {
                self.callsites
                    .push((callee, CallSiteLocation::Direct(location)));
            }
        }
        self.super_terminator(terminator, location);
    }

    /// Find where the closure is defined rather than called,
    /// including the closure instance and the arg.
    ///
    /// e.g., let mut _20: [closure@src/main.rs:13:28: 16:6];
    ///
    /// _20 is of type Closure, but it is actually the arg that captures
    /// the variables in the defining function.
    ///
    /// Each location where the closure may run is recorded.
    fn visit_local_decl(&mut self, local: Local, local_decl: &LocalDecl<'tcx>) {
        let func_ty = self.caller.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            self.typing_env,
            EarlyBinder::bind(self.tcx, local_decl.ty),
        );
        if let TyKind::Closure(def_id, substs) = func_ty.kind() {
            match self.body.local_kind(local) {
                LocalKind::Arg | LocalKind::ReturnPointer => {}
                _ => {
                    if let Some(callee_instance) =
                        Instance::try_resolve(self.tcx, self.typing_env, *def_id, substs)
                            .ok()
                            .flatten()
                    {
                        let calls = self.closure_run_locations(local);
                        if calls.is_empty() {
                            self.callsites
                                .push((callee_instance, CallSiteLocation::ClosureDef(local, None)));
                        }
                        for loc in calls {
                            self.callsites.push((
                                callee_instance,
                                CallSiteLocation::ClosureDef(local, Some(loc)),
                            ));
                        }
                    }
                }
            }
        }
        self.super_local_decl(local, local_decl);
    }
}

/// The function `func` calls in `caller`, once monomorphized, and its instance if resolved.
fn resolve_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    typing_env: TypingEnv<'tcx>,
    caller: Instance<'tcx>,
    body: &Body<'tcx>,
    func: &Operand<'tcx>,
) -> Option<(DefId, Option<Instance<'tcx>>)> {
    // Only after monomorphizing can Instance::try_resolve work
    let func_ty = caller.instantiate_mir_and_normalize_erasing_regions(
        tcx,
        typing_env,
        EarlyBinder::bind(tcx, func.ty(body, tcx)),
    );
    let ty::FnDef(def_id, substs) = *func_ty.kind() else {
        return None;
    };
    let callee = substs.no_bound_vars().and_then(|substs| {
        Instance::try_resolve(tcx, typing_env, def_id, substs)
            .ok()
            .flatten()
    });
    Some((def_id, callee))
}

/// How a call uses a value passed to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallUse {
    /// Stores or drops it.
    Store,
    /// Calls it, if it is a closure.
    Run,
}
impl CallUse {
    /// The use of a value passed to several calls in a function.
    fn combine(self, other: Self) -> Self {
        if self == Self::Run || other == Self::Run {
            Self::Run
        } else {
            Self::Store
        }
    }
}
/// Finds how functions use the values passed to them, following the calls they make.
/// A value passed to a function whose body is unknown is taken as run.
struct ArgUses<'tcx> {
    tcx: TyCtxt<'tcx>,
    typing_env: TypingEnv<'tcx>,
    arg_uses: FxHashMap<(Instance<'tcx>, usize), CallUse>,
    points_to: FxHashMap<Instance<'tcx>, PointsToMap<'tcx>>,
}

impl<'tcx> ArgUses<'tcx> {
    fn new(tcx: TyCtxt<'tcx>, typing_env: TypingEnv<'tcx>) -> Self {
        Self {
            tcx,
            typing_env,
            arg_uses: Default::default(),
            points_to: Default::default(),
        }
    }

    /// The calls in `instance` taking `local` or a value holding it, and how they use it.
    fn uses(&mut self, instance: Instance<'tcx>, local: Local) -> Vec<(Location, CallUse)> {
        let tcx = self.tcx;
        let body = tcx.instance_mir(instance.def);
        let points_to = self.points_to.entry(instance).or_insert_with(|| {
            let mut andersen = Andersen::new(body, tcx);
            andersen.analyze();
            andersen.finish()
        });
        let calls = body
            .basic_blocks
            .iter_enumerated()
            .filter_map(|(bb, data)| {
                let TerminatorKind::Call { func, args, .. } = &data.terminator().kind else {
                    return None;
                };
                let arg = args
                    .iter()
                    .position(|arg| holds(points_to, &arg.node, local))?;
                Some((body.terminator_loc(bb), func.clone(), arg))
            })
            .collect::<Vec<_>>();
        calls
            .into_iter()
            .map(|(location, func, arg)| (location, self.call_use(instance, body, &func, arg)))
            .collect()
    }

    /// How the call of `func` in `caller` uses its argument `arg`.
    fn call_use(
        &mut self,
        caller: Instance<'tcx>,
        body: &Body<'tcx>,
        func: &Operand<'tcx>,
        arg: usize,
    ) -> CallUse {
        // A call through a function pointer.
        let Some((def_id, callee)) = resolve_call(self.tcx, self.typing_env, caller, body, func)
        else {
            return CallUse::Run;
        };
        // `Fn::call`, `FnMut::call_mut` or `FnOnce::call_once` on the value.
        let is_fn_trait_call = self
            .tcx
            .opt_parent(def_id)
            .is_some_and(|parent| self.tcx.fn_trait_kind_from_def_id(parent).is_some());
        match callee {
            _ if arg == 0 && is_fn_trait_call => CallUse::Run,
            Some(callee) => self.arg_use(callee, arg),
            None => CallUse::Run,
        }
    }

    /// How `callee` uses its argument `arg`.
    fn arg_use(&mut self, callee: Instance<'tcx>, arg: usize) -> CallUse {
        if let Some(arg_use) = self.arg_uses.get(&(callee, arg)) {
            return *arg_use;
        }
        // A recursive call adds no use to the other calls of the function.
        self.arg_uses.insert((callee, arg), CallUse::Store);
        let arg_use = match callee.def {
            InstanceKind::Item(def_id) if self.tcx.is_mir_available(def_id) => self
                .uses(callee, Local::from_usize(arg + 1))
                .into_iter()
                .fold(CallUse::Store, |arg_use, (_, call_use)| {
                    arg_use.combine(call_use)
                }),
            // Intrinsics moving a value without running it.
            InstanceKind::Intrinsic(def_id)
                if matches!(
                    self.tcx.item_name(def_id).as_str(),
                    "box_new" | "write_via_move" | "forget" | "transmute" | "transmute_unchecked"
                ) =>
            {
                CallUse::Store
            }
            _ => CallUse::Run,
        };
        self.arg_uses.insert((callee, arg), arg_use);
        arg_use
    }
}

/// Check if `operand` holds `local` or a part of it, or points to a place holding one.
fn holds<'tcx>(points_to: &PointsToMap<'tcx>, operand: &Operand<'tcx>, local: Local) -> bool {
    let Some(place) = operand.place() else {
        return false;
    };
    let is_local = |node: &ConstraintNode<'tcx>| match node {
        ConstraintNode::Alloc(place) | ConstraintNode::Place(place) => place.local == local,
        _ => false,
    };
    let pointees = |place: PlaceRef<'tcx>| {
        points_to
            .get(&ConstraintNode::Place(place))
            .into_iter()
            .flatten()
    };
    place.local == local
        || pointees(place.as_ref()).any(|pointee| {
            is_local(pointee)
                || matches!(pointee, ConstraintNode::Place(place) if pointees(*place).any(is_local))
        })
}
