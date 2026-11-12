//! Asking a running command to stop.

use core::cell::Cell;
use core::task::{Poll, Waker};

/// What a driver raises to ask the running command to stop, such as when the host sends Ctrl-C. The driver
/// clears it before each command, and hands the command a [`Cancel`] from [`cancel`](Self::cancel).
///
/// It is not `Sync`. The driver raises it from the task that runs the command.
#[derive(Default)]
pub struct CancelSignal {
    raised: Cell<bool>,
    waker: Cell<Option<Waker>>,
}

impl CancelSignal {
    /// A signal that is not raised.
    pub const fn new() -> Self {
        Self {
            raised: Cell::new(false),
            waker: Cell::new(None),
        }
    }

    /// Asks the command to stop, and wakes it if it waits in [`Cancel::requested`].
    pub fn raise(&self) {
        self.raised.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    /// Withdraws the request, for the next command.
    pub fn clear(&self) {
        self.raised.set(false);
        self.waker.set(None);
    }

    /// The handle a command watches.
    pub fn cancel(&self) -> Cancel<'_> {
        Cancel(self)
    }
}

/// A running command's view of its [`CancelSignal`]. A command that can stop early, such as one that runs until
/// stopped, waits on [`requested`](Self::requested) beside its work, writes what it has and returns. A command
/// that does not watch is dropped at its next await if the driver gives up on it.
#[derive(Clone, Copy)]
pub struct Cancel<'a>(&'a CancelSignal);

impl Cancel<'_> {
    /// Whether the driver has asked the command to stop.
    pub fn is_requested(self) -> bool {
        self.0.raised.get()
    }

    /// Completes when the driver asks the command to stop, at once if it already has.
    pub async fn requested(self) {
        core::future::poll_fn(|context| {
            if self.is_requested() {
                Poll::Ready(())
            } else {
                self.0.waker.set(Some(context.waker().clone()));
                Poll::Pending
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::pin::pin;
    use core::task::Context;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    #[derive(Default)]
    struct Count(AtomicUsize);

    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_waiting_command_is_woken_by_a_raise_and_then_completes() {
        let signal = CancelSignal::new();
        let woken = Arc::new(Count::default());
        let waker = Waker::from(woken.clone());
        let mut context = Context::from_waker(&waker);
        let mut requested = pin!(signal.cancel().requested());
        assert_eq!(requested.as_mut().poll(&mut context), Poll::Pending);
        assert!(!signal.cancel().is_requested());
        signal.raise();
        assert_eq!(woken.0.load(Ordering::Relaxed), 1);
        assert!(signal.cancel().is_requested());
        assert_eq!(requested.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn a_cleared_signal_is_not_requested_and_its_old_waiter_is_not_woken() {
        let signal = CancelSignal::new();
        let woken = Arc::new(Count::default());
        let waker = Waker::from(woken.clone());
        let mut context = Context::from_waker(&waker);
        signal.raise();
        signal.clear();
        assert!(!signal.cancel().is_requested());
        let mut requested = pin!(signal.cancel().requested());
        assert_eq!(requested.as_mut().poll(&mut context), Poll::Pending);
        signal.clear();
        signal.raise();
        assert_eq!(woken.0.load(Ordering::Relaxed), 0);
    }
}
