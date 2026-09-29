use std::sync::{mpsc, Arc, Mutex};
use std::thread;

// Runs `f` through `dyn FnOnce`, which lockbud cannot follow.
fn run(f: Box<dyn FnOnce()>) {
    f()
}

fn run_ref(f: &dyn Fn()) {
    f()
}

struct Task {
    f: Box<dyn FnOnce()>,
}

impl Task {
    fn run(self) {
        (self.f)()
    }
}

// Deadlock: the closure runs while the lock is held.
fn held() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    run(Box::new(move || *a2.lock().unwrap() += 1));
}

// No deadlock: the lock is released before the closure is passed.
fn released() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    {
        let _guard = a.lock().unwrap();
    }
    run(Box::new(move || *a2.lock().unwrap() += 1));
}

// No deadlock: the lock is held only in the branch that does not run the closure.
fn other_branch(c: bool) {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    if c {
        let _guard = a.lock().unwrap();
    } else {
        run(Box::new(move || *a2.lock().unwrap() += 1));
    }
}

// Deadlock in one branch: the closure is passed in both, with the lock held in one.
fn two_calls(c: bool) {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let f = move || *a2.lock().unwrap() += 1;
    if c {
        let _guard = a.lock().unwrap();
        run(Box::new(f));
    } else {
        run(Box::new(f));
    }
}

// Deadlock: both closures run while the lock is held.
fn nested() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    run(Box::new(move || {
        run(Box::new(move || *a2.lock().unwrap() += 1));
    }));
}

// Deadlock: the innermost of three closures runs while the lock is held.
fn nested_three() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    run(Box::new(move || {
        run(Box::new(move || {
            run(Box::new(move || *a2.lock().unwrap() += 1));
        }));
    }));
}

// No deadlock: the closure runs after the function returns and releases the lock.
fn returned() -> impl FnOnce() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    move || *a2.lock().unwrap() += 1
}

// Deadlock: the closure boxed before the lock is taken runs while it is held.
fn boxed_before_lock() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let f: Box<dyn FnOnce()> = Box::new(move || *a2.lock().unwrap() += 1);
    let _guard = a.lock().unwrap();
    run(f);
}

// No deadlock: the closure boxed while the lock is held runs after it is released.
fn boxed_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let guard = a.lock().unwrap();
    let f: Box<dyn FnOnce()> = Box::new(move || *a2.lock().unwrap() += 1);
    drop(guard);
    run(f);
}

// No deadlock: the closure is dropped without running.
fn dropped() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let f: Box<dyn FnOnce()> = Box::new(move || *a2.lock().unwrap() += 1);
    let _guard = a.lock().unwrap();
    drop(f);
}

// Deadlock: the closure is passed by reference while the lock is held.
fn by_ref() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let f = move || *a2.lock().unwrap() += 1;
    let _guard = a.lock().unwrap();
    run_ref(&f);
}

// Deadlock: a struct holding the closure runs it while the lock is held.
fn in_struct() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let task = Task {
        f: Box::new(move || *a2.lock().unwrap() += 1),
    };
    let _guard = a.lock().unwrap();
    task.run();
}

// Deadlock: a generic function calls the closure while the lock is held.
fn generic() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    Some(1).map(move |_| *a2.lock().unwrap() += 1);
}

// No deadlock: the thread only waits for the lock.
fn spawned() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    thread::spawn(move || *a2.lock().unwrap() += 1);
}

// Deadlock: the thread is joined while the lock it waits for is held.
fn joined_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    let handle = thread::spawn(move || *a2.lock().unwrap() += 1);
    handle.join().unwrap();
}

// No deadlock: the lock is released before the thread is joined.
fn joined_after_release() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let guard = a.lock().unwrap();
    let handle = thread::spawn(move || *a2.lock().unwrap() += 1);
    drop(guard);
    handle.join().unwrap();
}

// Deadlock: same as `joined_while_locked`, with `thread::Builder`.
fn builder_joined_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    let handle = thread::Builder::new()
        .spawn(move || *a2.lock().unwrap() += 1)
        .unwrap();
    handle.join().unwrap();
}

// Deadlock: the scope joins its thread while the lock is held.
fn scope_while_locked() {
    let a = Mutex::new(0);
    let _guard = a.lock().unwrap();
    thread::scope(|s| {
        s.spawn(|| *a.lock().unwrap() += 1);
    });
}

// No deadlock: the lock taken in the scope is released before the scope joins its thread.
fn scope_released() {
    let a = Mutex::new(0);
    thread::scope(|s| {
        let guard = a.lock().unwrap();
        s.spawn(|| *a.lock().unwrap() += 1);
        drop(guard);
    });
}

// Deadlock: the scoped thread is joined while the lock is held.
fn scope_joined_while_locked() {
    let a = Mutex::new(0);
    thread::scope(|s| {
        let handle = s.spawn(|| *a.lock().unwrap() += 1);
        let _guard = a.lock().unwrap();
        handle.join().unwrap();
    });
}

fn spawn_task(f: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::spawn(f)
}

fn run_and_join(f: impl FnOnce() + Send + 'static) {
    thread::spawn(f).join().unwrap();
}

struct Queue {
    tasks: Vec<Box<dyn FnOnce()>>,
}

impl Queue {
    fn add(&mut self, f: Box<dyn FnOnce()>) {
        self.tasks.push(f);
    }
}

// No deadlock: the closure is pushed while the lock is held, and runs after it is released.
fn pushed_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let mut tasks: Vec<Box<dyn FnOnce()>> = Vec::new();
    let guard = a.lock().unwrap();
    tasks.push(Box::new(move || *a2.lock().unwrap() += 1));
    drop(guard);
    for task in tasks {
        task();
    }
}

// No deadlock: the closure is queued while the lock is held, and never run here.
fn queued_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let mut queue = Queue { tasks: Vec::new() };
    let _guard = a.lock().unwrap();
    queue.add(Box::new(move || *a2.lock().unwrap() += 1));
}

// No deadlock: the closure is sent to a thread, which waits for the lock.
fn sent_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    thread::spawn(move || receiver.recv().unwrap()());
    let _guard = a.lock().unwrap();
    sender
        .send(Box::new(move || *a2.lock().unwrap() += 1))
        .unwrap();
}

// No deadlock: a helper spawns the thread, which only waits for the lock.
fn spawned_by_helper() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    spawn_task(move || *a2.lock().unwrap() += 1);
}

// Deadlock: the thread a helper spawns is joined while the lock is held.
fn helper_joined_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    spawn_task(move || *a2.lock().unwrap() += 1).join().unwrap();
}

// Deadlock: a helper spawns the thread and waits for it while the lock is held.
fn helper_waits_while_locked() {
    let a = Arc::new(Mutex::new(0));
    let a2 = a.clone();
    let _guard = a.lock().unwrap();
    run_and_join(move || *a2.lock().unwrap() += 1);
}

fn main() {
    held();
    released();
    other_branch(true);
    two_calls(true);
    nested();
    nested_three();
    returned()();
    boxed_before_lock();
    boxed_while_locked();
    dropped();
    by_ref();
    in_struct();
    generic();
    spawned();
    joined_while_locked();
    joined_after_release();
    builder_joined_while_locked();
    scope_while_locked();
    scope_released();
    scope_joined_while_locked();
    pushed_while_locked();
    queued_while_locked();
    sent_while_locked();
    spawned_by_helper();
    helper_joined_while_locked();
    helper_waits_while_locked();
}
