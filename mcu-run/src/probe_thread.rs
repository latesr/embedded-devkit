//! One thread for every probe-rs call that lists or opens probes.
//!
//! probe-rs lists CMSIS-DAP v1 probes through hidapi, even when none is attached. On macOS, hidapi ties its one
//! device manager to the run loop of the first thread that lists. When that thread ends, its run loop is freed,
//! and a later listing on another thread can schedule a device on the freed run loop, which traps the process in
//! CoreFoundation. A test harness runs each test on a new thread, so a scenario test that flashed a board after
//! another test had flashed one could crash. probe-rs-espressif enables probe-rs's default features, so
//! `cmsisdap_v1` cannot be turned off. Every listing and open therefore runs on one thread that lives as long as
//! the process.

use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

type Job = Box<dyn FnOnce() + Send>;

/// The probe thread's queue, or `None` when the thread could not be started.
static JOBS: OnceLock<Option<Sender<Job>>> = OnceLock::new();

/// Runs `job` on the probe thread and returns its result. A panic in `job` resumes on the caller's thread. When
/// the probe thread could not be started, `job` runs on the caller's thread.
pub(crate) fn on_probe_thread<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> T {
    let jobs = JOBS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("mcu-run probes".to_owned())
            .spawn(move || receiver.into_iter().for_each(|job| job()))
            .ok()
            .map(|_| sender)
    });
    let (done, result) = mpsc::sync_channel(1);
    let run: Job = Box::new(move || {
        let _ = done.send(panic::catch_unwind(AssertUnwindSafe(job)));
    });
    let unsent = match jobs {
        Some(jobs) => jobs.send(run).err().map(|error| error.0),
        None => Some(run),
    };
    if let Some(run) = unsent {
        run();
    }
    match result.recv() {
        Ok(Ok(value)) => value,
        Ok(Err(payload)) => panic::resume_unwind(payload),
        // The job always answers, because it catches its own panic, so this needs the probe thread to have died.
        Err(_) => panic::resume_unwind(Box::new("the mcu-run probe thread stopped")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::{self, ThreadId};

    #[test]
    fn jobs_from_threads_that_have_ended_run_on_one_thread_that_has_not() {
        let caller = || {
            let me = thread::current().id();
            let runner: ThreadId = on_probe_thread(|| thread::current().id());
            (me, runner)
        };
        let (first_caller, first_runner) = thread::spawn(caller).join().unwrap();
        let (second_caller, second_runner) = thread::spawn(caller).join().unwrap();
        assert_eq!(first_runner, second_runner);
        assert_ne!(first_runner, first_caller);
        assert_ne!(second_runner, second_caller);
    }

    #[test]
    fn a_panic_in_the_job_reaches_the_caller_and_the_thread_goes_on() {
        let panicked = std::panic::catch_unwind(|| on_probe_thread(|| panic!("in the job")));
        assert_eq!(
            panicked.unwrap_err().downcast_ref::<&str>(),
            Some(&"in the job")
        );
        assert_eq!(on_probe_thread(|| 7), 7);
    }
}
