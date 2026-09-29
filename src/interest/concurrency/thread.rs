//! Denotes thread APIs in std.
//!
//! 1. std::thread::spawn(F), std::thread::Builder::spawn(Builder, F), std::thread::Builder::spawn_unchecked(Builder, F)
//! 2. std::thread::Scope::spawn(&Scope, F), std::thread::Builder::spawn_scoped(Builder, &Scope, F)
//! 3. std::thread::JoinHandle::join(JoinHandle), std::thread::ScopedJoinHandle::join(ScopedJoinHandle)
extern crate rustc_hir;

use once_cell::sync::Lazy;
use regex::Regex;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadApi {
    /// Runs its last argument, a closure, on a new thread.
    Spawn,
    /// Same, on a thread joined when its `std::thread::scope` returns.
    SpawnScoped,
    /// Waits for the thread of its argument.
    Join,
}

static SPAWN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^std::thread::(spawn|Builder::spawn(_unchecked)?)$").unwrap());
static SPAWN_SCOPED: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^std::thread::(Scope(::<.*>)?::spawn|(scoped::<impl std::thread::Builder>|Builder)::spawn_scoped)$")
        .unwrap()
});
static JOIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^std::thread::(Scoped)?JoinHandle(::<.*>)?::join$").unwrap());

impl ThreadApi {
    pub fn from_def_id(def_id: DefId, tcx: TyCtxt<'_>) -> Option<Self> {
        Self::from_path(&tcx.def_path_str(def_id))
    }

    fn from_path(path: &str) -> Option<Self> {
        if SPAWN.is_match(path) {
            Some(Self::Spawn)
        } else if SPAWN_SCOPED.is_match(path) {
            Some(Self::SpawnScoped)
        } else if JOIN.is_match(path) {
            Some(Self::Join)
        } else {
            None
        }
    }

    pub fn is_spawn(self) -> bool {
        matches!(self, Self::Spawn | Self::SpawnScoped)
    }
}

#[cfg(test)]
mod tests {
    use super::ThreadApi;

    #[test]
    fn test_thread_api_from_path() {
        for path in [
            "std::thread::spawn",
            "std::thread::Builder::spawn",
            "std::thread::Builder::spawn_unchecked",
        ] {
            assert_eq!(ThreadApi::from_path(path), Some(ThreadApi::Spawn), "{path}");
        }
        for path in [
            "std::thread::Scope::<'scope, 'env>::spawn",
            "std::thread::scoped::<impl std::thread::Builder>::spawn_scoped",
        ] {
            assert_eq!(
                ThreadApi::from_path(path),
                Some(ThreadApi::SpawnScoped),
                "{path}"
            );
        }
        for path in [
            "std::thread::JoinHandle::<T>::join",
            "std::thread::ScopedJoinHandle::<'scope, T>::join",
        ] {
            assert_eq!(ThreadApi::from_path(path), Some(ThreadApi::Join), "{path}");
        }
        for path in [
            "std::thread::scope",
            "std::thread::Builder::new",
            "std::thread::lifecycle::spawn_unchecked",
            "std::thread::JoinHandle::<T>::thread",
        ] {
            assert_eq!(ThreadApi::from_path(path), None, "{path}");
        }
    }
}
