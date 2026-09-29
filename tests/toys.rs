//! Runs lockbud on the crates in `toys/` and compares its reports with the expected ones.
//!
//! Each report is summarized on one line. A source location is its line in `src/main.rs`,
//! `<file>:<line>` in another file of the toy, or `ext` outside the toy.
//! - `DoubleLock Probably 5 StdMutex(i32) -> 6 StdMutex(i32) via 9 > 12,14`, where `via` lists
//!   the calls leading from the first lock to the second one: `9`, then `12` or `14`,
//! - `ConflictLock Possibly <lock pair>; <lock pair>`,
//! - `CondvarDeadlock Possibly wait 69, notify 78: 65 StdMutex(i32) / 74 StdMutex(i32)`,
//! - `AtomicityViolation Possibly <fn>: <read> -> <write> <dependency>`,
//! - `UseAfterFree Possibly <description>: <locations>`,
//! - `Panic <fn>: <panic API>`.
use std::path::Path;
use std::process::Command;

use regex::Regex;
use serde_json::Value;

/// Runs the `kind` detector on `toys/<toy>` and returns the summaries of its reports, sorted.
fn detect(toy: &str, kind: &str) -> Vec<String> {
    detect_with_flags(toy, &format!("-k {kind} -l {}", toy.replace('-', "_")))
}

fn detect_with_flags(toy: &str, flags: &str) -> Vec<String> {
    let toy_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("toys").join(toy);
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("toys")
        .join(toy);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    // Lockbud only analyzes the crates cargo compiles again.
    let clean = Command::new(&cargo)
        .args(["clean", "-p", toy])
        .current_dir(&toy_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .unwrap();
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let build = Command::new(&cargo)
        .args(["build", "--locked"])
        .current_dir(&toy_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("RUSTC_WRAPPER", env!("CARGO_BIN_EXE_lockbud"))
        .env("LOCKBUD_LOG", "info")
        .env("LOCKBUD_FLAGS", flags)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(build.status.success(), "{stderr}");
    let mut summaries = summarize(&stderr);
    summaries.extend(summarize_panics(&String::from_utf8_lossy(&build.stdout)));
    summaries.sort();
    summaries
}

/// Summarizes the reports that lockbud logs as JSON arrays.
fn summarize(stderr: &str) -> Vec<String> {
    let marker = "WARN  lockbud::callbacks] [";
    stderr
        .match_indices(marker)
        .flat_map(|(start, _)| {
            let json = &stderr[start + marker.len() - 1..];
            match serde_json::Deserializer::from_str(json)
                .into_iter::<Value>()
                .next()
            {
                Some(Ok(Value::Array(reports))) => reports,
                other => panic!("invalid report {other:?} in:\n{stderr}"),
            }
        })
        .map(|report| summarize_report(&report))
        .collect()
}

fn summarize_report(report: &Value) -> String {
    let (kind, content) = report.as_object().unwrap().iter().next().unwrap();
    let diagnosis = &content["diagnosis"];
    let detail = match kind.as_str() {
        "DoubleLock" => summarize_deadlock(diagnosis),
        "ConflictLock" => {
            let mut deadlocks = diagnosis
                .as_array()
                .unwrap()
                .iter()
                .map(summarize_deadlock)
                .collect::<Vec<_>>();
            deadlocks.sort();
            deadlocks.join("; ")
        }
        "CondvarDeadlock" => {
            let mut deadlocks = diagnosis["deadlocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|locks| {
                    format!(
                        "{} / {}",
                        lock(&locks["wait_lock_span"], &locks["wait_lock_type"]),
                        lock(&locks["notify_lock_span"], &locks["notify_lock_type"])
                    )
                })
                .collect::<Vec<_>>();
            deadlocks.sort();
            format!(
                "wait {}, notify {}: {}",
                location(&diagnosis["condvar_wait_callsite_span"]),
                location(&diagnosis["condvar_notify_callsite_span"]),
                deadlocks.join("; ")
            )
        }
        "AtomicityViolation" => format!(
            "{}: {} -> {} {}",
            diagnosis["fn_name"].as_str().unwrap(),
            location(&diagnosis["atomic_reader"]),
            location(&diagnosis["atomic_writer"]),
            diagnosis["dep_kind"].as_str().unwrap()
        ),
        _ => {
            // Memory bugs are described in text, with the MIR places that change with rustc.
            let text = diagnosis.as_str().unwrap();
            let lines = Regex::new(r"src/main\.rs:(\d+):")
                .unwrap()
                .captures_iter(text)
                .map(|captures| captures[1].to_owned())
                .collect::<Vec<_>>();
            let description = text
                .split(':')
                .next()
                .unwrap()
                .split(" at ")
                .next()
                .unwrap();
            format!("{description}: {}", lines.join(" -> "))
        }
    };
    format!(
        "{kind} {} {detail}",
        content["possibility"].as_str().unwrap()
    )
}

fn summarize_deadlock(diagnosis: &Value) -> String {
    let deadlock = format!(
        "{} -> {}",
        lock(&diagnosis["first_lock_span"], &diagnosis["first_lock_type"]),
        lock(
            &diagnosis["second_lock_span"],
            &diagnosis["second_lock_type"]
        )
    );
    // Only the calls in the toy: those inside std change with rustc.
    let mut chains = diagnosis["callchains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chain| {
            chain
                .as_array()
                .unwrap()
                .iter()
                .map(|callsites| {
                    callsites
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(location)
                        .filter(|location| location != "ext")
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|callsites| !callsites.is_empty())
                .collect::<Vec<_>>()
                .join(" > ")
        })
        .filter(|chain| !chain.is_empty())
        .collect::<Vec<_>>();
    chains.sort();
    chains.dedup();
    if chains.is_empty() {
        deadlock
    } else {
        format!("{deadlock} via {}", chains.join(" | "))
    }
}

fn lock(span: &Value, ty: &Value) -> String {
    format!("{} {}", location(span), ty.as_str().unwrap())
}

/// The location of a span such as `src/main.rs:77:16: 77:32 (#0)`.
fn location(span: &Value) -> String {
    let span = span.as_str().unwrap();
    let mut parts = span.split(':');
    let (file, line) = (parts.next().unwrap(), parts.next().unwrap_or_default());
    if file == "src/main.rs" {
        line.to_owned()
    } else if file.starts_with('/') || file.starts_with('<') {
        "ext".to_owned()
    } else {
        format!("{file}:{line}")
    }
}

/// Summarizes the panic locations printed by the panic detector.
fn summarize_panics(stdout: &str) -> Vec<String> {
    let panic = Regex::new(r"^PANIC\[.*?::(\w+)\), .*\], (\w+)").unwrap();
    stdout
        .lines()
        .filter_map(|line| panic.captures(line))
        .map(|captures| format!("Panic {}: {}", &captures[1], &captures[2]))
        .collect()
}

// Toys shipped with lockbud. The expected reports are those of upstream lockbud on
// nightly-2026-02-07, except where noted.

#[test]
fn atomic_violation() {
    assert_eq!(
        detect("atomic-violation", "atomicity_violation"),
        [
            "AtomicityViolation Possibly buggy_both_dep_i32: 42 -> 46 Both",
            "AtomicityViolation Possibly buggy_control_dep_bool: 14 -> 15 Control",
            "AtomicityViolation Possibly buggy_control_dep_i32: 22 -> 26 Control",
            "AtomicityViolation Possibly buggy_data_dep_i32: 33 -> 36 Data",
        ]
    );
}

#[test]
fn call_no_deadlock() {
    assert!(detect("call-no-deadlock", "deadlock").is_empty());
}

#[test]
fn condvar_closure() {
    assert!(detect("condvar-closure", "deadlock").is_empty());
}

#[test]
fn condvar_struct() {
    assert_eq!(
        detect("condvar-struct", "deadlock"),
        ["CondvarDeadlock Possibly wait 69, notify 78: 65 StdMutex(i32) / 74 StdMutex(i32)"]
    );
}

#[test]
fn conflict() {
    assert_eq!(
        detect("conflict", "deadlock"),
        [
            "ConflictLock Possibly 21 StdMutex(bool) -> 26 StdRwLockRead(i32); \
          38 StdRwLockWrite(i32) -> 42 StdRwLockRead(u8); \
          50 StdRwLockWrite(u8) -> 54 StdMutex(bool)"
        ]
    );
}

#[test]
fn conflict_inter() {
    assert_eq!(
        detect("conflict-inter", "deadlock"),
        [
            "ConflictLock Possibly 18 StdMutex(i32) -> 25 StdRwLockWrite(i32) via 20; \
          29 StdRwLockRead(i32) -> 36 StdMutex(i32) via 31"
        ]
    );
}

#[test]
fn inter() {
    assert_eq!(
        detect("inter", "deadlock"),
        [
            "DoubleLock Possibly 107 SpinRead(i32) -> 125 SpinWrite(i32) via 108",
            "DoubleLock Possibly 114 SpinWrite(i32) -> 121 SpinRead(i32) via 116",
            "DoubleLock Possibly 114 SpinWrite(i32) -> 125 SpinWrite(i32) via 115",
            "DoubleLock Possibly 25 StdMutex(i32) -> 33 StdMutex(i32) via 28",
            "DoubleLock Possibly 37 StdRwLockRead(i32) -> 51 StdRwLockRead(i32) via 39",
            "DoubleLock Possibly 37 StdRwLockRead(i32) -> 55 StdRwLockWrite(i32) via 38",
            "DoubleLock Possibly 44 StdRwLockWrite(i32) -> 51 StdRwLockRead(i32) via 46",
            "DoubleLock Possibly 44 StdRwLockWrite(i32) -> 55 StdRwLockWrite(i32) via 45",
            "DoubleLock Possibly 59 ParkingLotMutex(i32) -> 66 ParkingLotMutex(i32) via 61",
            "DoubleLock Possibly 70 ParkingLotRead(i32) -> 84 ParkingLotRead(i32) via 72",
            "DoubleLock Possibly 70 ParkingLotRead(i32) -> 88 ParkingLotWrite(i32) via 71",
            "DoubleLock Possibly 77 ParkingLotWrite(i32) -> 84 ParkingLotRead(i32) via 79",
            "DoubleLock Possibly 77 ParkingLotWrite(i32) -> 88 ParkingLotWrite(i32) via 78",
            "DoubleLock Possibly 92 SpinMutex(i32) -> 103 SpinMutex(i32) via 94",
        ]
    );
}

#[test]
fn intra() {
    assert_eq!(
        detect("intra", "deadlock"),
        [
            "DoubleLock Possibly 15 StdRwLockRead(i32) -> 17 StdRwLockRead(i32)",
            "DoubleLock Possibly 33 ParkingLotRead(i32) -> 35 ParkingLotRead(i32)",
            "DoubleLock Probably 15 StdRwLockRead(i32) -> 16 StdRwLockWrite(i32)",
            "DoubleLock Probably 24 ParkingLotMutex(i32) -> 26 ParkingLotMutex(i32)",
            "DoubleLock Probably 33 ParkingLotRead(i32) -> 34 ParkingLotWrite(i32)",
            "DoubleLock Probably 6 StdMutex(i32) -> 8 StdMutex(i32)",
        ]
    );
}

#[test]
fn invalid_free() {
    assert!(detect("invalid-free", "memory").is_empty());
}

/// Upstream also reports two double locks and two conflicts through `thread::spawn`
/// at line 9, with locks the main thread only takes after spawning.
#[test]
fn issue71() {
    assert_eq!(
        detect("issue71", "deadlock"),
        [
            "ConflictLock Possibly 10 StdMutex(bool) -> 11 StdMutex(bool); \
          16 StdMutex(bool) -> 17 StdMutex(bool)"
        ]
    );
}

/// Upstream also reports two double locks and two conflicts through `thread::spawn`
/// at line 13, with locks released before spawning.
#[test]
fn lock_closure() {
    assert_eq!(
        detect("lock-closure", "deadlock"),
        [
            "ConflictLock Possibly 10 StdMutex(bool) -> 11 StdMutex(i32); \
             14 StdMutex(i32) -> 15 StdMutex(bool)",
            "ConflictLock Possibly 26 StdMutex(bool) -> 27 StdMutex(i32); \
             30 StdMutex(i32) -> 31 StdMutex(bool)",
        ]
    );
}

#[test]
fn panic() {
    assert_eq!(
        detect("panic", "panic"),
        [
            "Panic assert_panic: AssertFailed",
            "Panic expect_panic: ResultExpect",
            "Panic panic_macro: PanicFmt",
            "Panic unwrap_panic: OptionUnwrap",
        ]
    );
}

#[test]
fn recursive_no_deadlock() {
    assert_eq!(
        detect("recursive-no-deadlock", "deadlock"),
        ["DoubleLock Possibly 6 ParkingLotRead(i32) -> 8 ParkingLotRead(i32)"]
    );
}

#[test]
fn static_ref() {
    assert_eq!(
        detect("static-ref", "deadlock"),
        ["DoubleLock Probably 11 StdMutex(bool) -> 12 StdMutex(bool)"]
    );
}

#[test]
fn tikv_wrapper() {
    assert_eq!(
        detect("tikv-wrapper", "deadlock"),
        ["DoubleLock Probably 18 StdRwLockRead(i32) -> 19 StdRwLockWrite(i32)"]
    );
}

#[test]
fn use_after_free() {
    assert_eq!(
        detect("use-after-free", "memory"),
        [
            "UseAfterFree Possibly Escape to Global: 96 -> 102",
            "UseAfterFree Possibly Escape to Param/Return: 113 -> 131",
            "UseAfterFree Possibly Escape to Param/Return: 46 -> 47",
            "UseAfterFree Possibly Raw ptr is used: 131 -> 131",
            "UseAfterFree Possibly Raw ptr is used: 14 -> 10",
            "UseAfterFree Possibly Raw ptr is used: 14 -> 10",
            "UseAfterFree Possibly Raw ptr is used: 15 -> 10",
        ]
    );
}

#[test]
fn wait_lock_no_deadlock() {
    assert!(detect("wait-lock-no-deadlock", "deadlock").is_empty());
}

// Toys for the changes of this fork.

/// Build scripts are skipped, whatever cargo names their output directory.
#[test]
fn build_script() {
    assert_eq!(
        detect_with_flags("build-script", "-k deadlock"),
        ["DoubleLock Probably 5 StdMutex(i32) -> 6 StdMutex(i32)"]
    );
}

/// The library and binary crates of `build-script-custom` are named `build_script_custom`.
#[test]
fn build_script_custom() {
    assert_eq!(
        detect_with_flags("build-script-custom", "-k deadlock"),
        [
            "DoubleLock Probably 5 StdMutex(i32) -> 6 StdMutex(i32)",
            "DoubleLock Probably src/lib.rs:4 StdMutex(i32) -> src/lib.rs:5 StdMutex(i32)",
        ]
    );
}

/// A closure gets the locks held where it may run: at the calls running it or a value holding
/// it, and for a thread, where it is joined. A call only storing it, like `Vec::push`, does not
/// run it. Each function of the toy says if it deadlocks.
#[test]
fn closure_call_site() {
    assert_eq!(
        detect("closure-call-site", "deadlock"),
        [
            "DoubleLock Possibly 100 StdMutex(i32) -> 99 StdMutex(i32) via 101",
            "DoubleLock Possibly 128 StdMutex(i32) -> 127 StdMutex(i32) via 129",
            "DoubleLock Possibly 139 StdMutex(i32) -> 137 StdMutex(i32) via 140",
            "DoubleLock Possibly 147 StdMutex(i32) -> 148 StdMutex(i32) via 148",
            "DoubleLock Possibly 163 StdMutex(i32) -> 164 StdMutex(i32) via 165",
            "DoubleLock Possibly 182 StdMutex(i32) -> 184 StdMutex(i32) via 186",
            "DoubleLock Possibly 27 StdMutex(i32) -> 28 StdMutex(i32) via 28",
            "DoubleLock Possibly 282 StdMutex(i32) -> 283 StdMutex(i32) via 283",
            "DoubleLock Possibly 290 StdMutex(i32) -> 291 StdMutex(i32) via 291",
            "DoubleLock Possibly 58 StdMutex(i32) -> 56 StdMutex(i32) via 59,61",
            "DoubleLock Possibly 69 StdMutex(i32) -> 71 StdMutex(i32) via 70 > 71",
            "DoubleLock Possibly 79 StdMutex(i32) -> 82 StdMutex(i32) via 80 > 81 > 82",
            "DoubleLock Probably 192 StdMutex(i32) -> 194 StdMutex(i32) via 193 > 194",
            "DoubleLock Probably 213 StdMutex(i32) -> 212 StdMutex(i32) via 212,214",
        ]
    );
}

/// A closure locking a mutex reached through its captures, up through enclosing closures.
#[test]
fn closure_ref_upvar() {
    assert_eq!(
        detect("closure-ref-upvar", "deadlock"),
        [
            "DoubleLock Possibly 62 StdMutex(i32) -> 63 StdMutex(i32) via 63",
            "DoubleLock Possibly 74 StdMutex(i32) -> 97 StdMutex(i32) via 75 > 75",
            "DoubleLock Possibly 86 StdMutex(i32) -> 87 StdMutex(i32) via 87 > 87",
            "DoubleLock Probably 10 StdMutex(i32) -> 11 StdMutex(i32) via 11",
            "DoubleLock Probably 16 StdMutex(i32) -> 17 StdMutex(i32) via 18",
            "DoubleLock Probably 37 StdMutex(i32) -> 38 StdMutex(i32) via 38",
            "DoubleLock Probably 50 StdMutex(i32) -> 51 StdMutex(i32) via 51",
        ]
    );
}
