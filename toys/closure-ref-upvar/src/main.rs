use std::sync::{Arc, Mutex};

// Runs `f` through `dyn FnOnce`, which lockbud cannot follow.
fn run<'a>(f: Box<dyn FnOnce() + 'a>) {
    f()
}

// Deadlock: the closure locks the `&Mutex` parameter while it is locked.
fn ref_param(m: &Mutex<i32>) {
    let _guard = m.lock().unwrap();
    run(Box::new(|| *m.lock().unwrap() += 1));
}

// Deadlock: same, with the closure called directly.
fn ref_param_direct(m: &Mutex<i32>) {
    let _guard = m.lock().unwrap();
    let f = || *m.lock().unwrap() += 1;
    f();
}

// No deadlock: the closure locks another parameter.
fn ref_param_other(m: &Mutex<i32>, n: &Mutex<i32>) {
    let _guard = m.lock().unwrap();
    run(Box::new(|| *n.lock().unwrap() += 1));
}

// No deadlock: the lock is released before the closure is passed.
fn ref_param_released(m: &Mutex<i32>) {
    {
        let _guard = m.lock().unwrap();
    }
    run(Box::new(|| *m.lock().unwrap() += 1));
}

// Deadlock: the closure locks the `&Arc<Mutex>` parameter while it is locked.
fn arc_ref_param(a: &Arc<Mutex<i32>>) {
    let _guard = a.lock().unwrap();
    run(Box::new(|| *a.lock().unwrap() += 1));
}

// No deadlock: the closure locks another parameter.
fn arc_ref_param_other(a: &Arc<Mutex<i32>>, b: &Arc<Mutex<i32>>) {
    let _guard = a.lock().unwrap();
    run(Box::new(|| *b.lock().unwrap() += 1));
}

// Deadlock: the closure borrows a local mutex while it is locked.
fn local() {
    let m = Mutex::new(0);
    let _guard = m.lock().unwrap();
    run(Box::new(|| *m.lock().unwrap() += 1));
}

struct App {
    a: Arc<Mutex<i32>>,
    b: Arc<Mutex<i32>>,
}

impl App {
    // Deadlock: the closure borrows `self` and locks the locked field.
    fn same_field(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| *self.a.lock().unwrap() += 1));
    }

    // No deadlock: the closure locks another field.
    fn other_field(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| *self.b.lock().unwrap() += 1));
    }

    // Deadlock: the closure calls a method locking the locked field.
    fn same_field_method(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| self.lock_a()));
    }

    // No deadlock: the method locks another field.
    fn other_field_method(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| self.lock_b()));
    }

    // Deadlock: nested closures borrow `self`, as egui layouts do.
    fn nested_same_field(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| run(Box::new(|| *self.a.lock().unwrap() += 1))));
    }

    // No deadlock: the nested closure locks another field.
    fn nested_other_field(&self) {
        let _guard = self.a.lock().unwrap();
        run(Box::new(|| run(Box::new(|| *self.b.lock().unwrap() += 1))));
    }

    fn lock_a(&self) {
        *self.a.lock().unwrap() += 1;
    }

    fn lock_b(&self) {
        *self.b.lock().unwrap() += 1;
    }
}

fn main() {
    let (m, n) = (Mutex::new(0), Mutex::new(0));
    ref_param(&m);
    ref_param_direct(&m);
    ref_param_other(&m, &n);
    ref_param_released(&m);
    let (a, b) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
    arc_ref_param(&a);
    arc_ref_param_other(&a, &b);
    local();
    let app = App { a, b };
    app.same_field();
    app.other_field();
    app.same_field_method();
    app.other_field_method();
    app.nested_same_field();
    app.nested_other_field();
}
