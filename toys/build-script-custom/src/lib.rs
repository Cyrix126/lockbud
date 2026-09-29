use std::sync::Mutex;

pub fn double_lock(m: &Mutex<i32>) {
    let _a = m.lock().unwrap();
    let _b = m.lock().unwrap();
}
