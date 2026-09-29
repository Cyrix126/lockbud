use std::sync::Mutex;

fn main() {
    let m = Mutex::new(0);
    let _a = m.lock().unwrap();
    let _b = m.lock().unwrap();
}
