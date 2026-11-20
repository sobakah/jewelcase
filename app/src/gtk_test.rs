//! Tests that use GTK run on one thread: GTK is started once and only
//! works on the thread it was started on, while Rust runs tests on
//! several. Each test starting it itself skipped on all threads but the
//! first, and two starting it at once could crash the run.

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;

type Job = Box<dyn FnOnce() + Send>;
type Done = mpsc::Sender<thread::Result<()>>;
type Queue = Mutex<mpsc::Sender<(Job, Done)>>;

/// The GTK thread's queue; none without a display.
fn queue() -> Option<&'static Queue> {
    static QUEUE: OnceLock<Option<Queue>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (started, ready) = mpsc::channel();
            let (jobs, incoming) = mpsc::channel::<(Job, Done)>();
            thread::Builder::new()
                .name("gtk-tests".into())
                .spawn(move || {
                    let ok = adw::init().is_ok();
                    let _ = started.send(ok);
                    if !ok {
                        return;
                    }
                    for (job, done) in incoming {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        let _ = done.send(result);
                    }
                })
                .expect("a thread for GTK");
            ready.recv().unwrap_or(false).then(|| Mutex::new(jobs))
        })
        .as_ref()
}

/// Runs `test` on the GTK thread (a failure there fails the test); without
/// a display it's skipped.
pub fn run(test: impl FnOnce() + Send + 'static) {
    let Some(queue) = queue() else {
        eprintln!("no display: skipped");
        return;
    };
    let (done, result) = mpsc::channel();
    queue
        .lock()
        .expect("the GTK queue")
        .send((Box::new(test), done))
        .expect("the GTK thread runs");
    if let Err(panic) = result.recv().expect("the GTK thread answers") {
        std::panic::resume_unwind(panic);
    }
}
