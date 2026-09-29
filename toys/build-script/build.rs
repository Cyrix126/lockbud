use std::sync::Mutex;

// Not reported: build scripts are not analyzed.
fn double_lock() {
    let m = Mutex::new(0);
    let _a = m.lock().unwrap();
    let _b = m.lock().unwrap();
}

fn main() {
    // Reachable, so it is compiled, but never run.
    if std::env::args().count() > 100 {
        double_lock();
    }
}
