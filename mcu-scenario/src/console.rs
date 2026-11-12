//! The console of a started firmware, held for one test.

use crate::Error;
use mcu_run::{Client, Reply, Transport};
use std::ops::{Deref, DerefMut};
use std::sync::MutexGuard;
use std::time::Duration;

/// How long a quick command may take to answer.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The console of a started firmware. It holds the board until it is dropped, so no other test in the process
/// starts firmware on it meanwhile. A helper takes it as `&mut Console`.
///
/// It dereferences to `mcu-run`'s [`Client`], for [`Client::wait_for_banner`] and [`Client::send_interrupted`].
/// Its own [`send`](Self::send) takes a default timeout in place of the client's.
pub struct Console {
    client: Client<Transport>,
    _board: MutexGuard<'static, ()>,
}

impl Console {
    pub(crate) fn new(client: Client<Transport>, board: MutexGuard<'static, ()>) -> Self {
        Self {
            client,
            _board: board,
        }
    }

    /// Sends `line` and returns its reply. Fails when no result line arrives within 5 s.
    pub fn send(&mut self, line: &str) -> Result<Reply, Error> {
        self.send_within(line, TIMEOUT)
    }

    /// Sends `line` and returns its reply. Fails when no result line arrives within `timeout`, for a command that
    /// takes longer than [`send`](Self::send) waits.
    pub fn send_within(&mut self, line: &str, timeout: Duration) -> Result<Reply, Error> {
        Ok(self.client.send(line, timeout)?)
    }

    /// Sends `line` again and again until `found` takes a value from its reply, and returns that value. A test
    /// waits this way for a state, such as a network lease in `net config`.
    ///
    /// Each send waits for its reply at most 5 s, or the time left when that is shorter. The next send follows
    /// 250 ms after a reply. A reply with an `err` result goes to `found` like any other.
    ///
    /// # Errors
    ///
    /// [`Error::NotReached`], with the last reply, when `found` takes nothing within `timeout`. [`Error::Run`] at
    /// once when a send fails.
    pub fn send_until<T>(
        &mut self,
        line: &str,
        timeout: Duration,
        found: impl FnMut(&Reply) -> Option<T>,
    ) -> Result<T, Error> {
        let start = std::time::Instant::now();
        poll(
            |wait| self.send_within(line, wait),
            line,
            timeout,
            found,
            || start.elapsed(),
            || std::thread::sleep(PAUSE),
        )
    }
}

impl Deref for Console {
    type Target = Client<Transport>;

    fn deref(&self) -> &Client<Transport> {
        &self.client
    }
}

impl DerefMut for Console {
    fn deref_mut(&mut self) -> &mut Client<Transport> {
        &mut self.client
    }
}

/// How long [`Console::send_until`] waits after a reply before it sends again.
const PAUSE: Duration = Duration::from_millis(250);

/// The loop of [`Console::send_until`]. `send` sends the line with the wait it is given, `elapsed` gives the time
/// since the start, and `pause` waits between sends.
fn poll<T>(
    mut send: impl FnMut(Duration) -> Result<Reply, Error>,
    line: &str,
    timeout: Duration,
    mut found: impl FnMut(&Reply) -> Option<T>,
    mut elapsed: impl FnMut() -> Duration,
    mut pause: impl FnMut(),
) -> Result<T, Error> {
    let mut last = None;
    loop {
        let left = timeout.saturating_sub(elapsed());
        if left.is_zero() {
            return Err(Error::NotReached {
                line: line.to_owned(),
                waited: timeout,
                last,
            });
        }
        let reply = send(left.min(TIMEOUT))?;
        if let Some(value) = found(&reply) {
            return Ok(value);
        }
        last = Some(reply);
        pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcu_run::CommandResult;
    use std::cell::{Cell, RefCell};

    fn reply(link: &str) -> Reply {
        Reply {
            body: vec![format!("link: {link}")],
            result: CommandResult::Ok,
        }
    }

    #[test]
    fn the_line_is_sent_until_the_reply_gives_a_value_and_that_value_is_returned() {
        let replies = RefCell::new(vec![reply("up"), reply("down"), reply("down")]);
        let now = Cell::new(Duration::ZERO);
        let paused = Cell::new(0);
        let found = poll(
            |_| Ok(replies.borrow_mut().pop().unwrap()),
            "net config",
            Duration::from_secs(20),
            |reply| (reply.field("link") == Some("up")).then_some("leased"),
            || now.get(),
            || {
                paused.set(paused.get() + 1);
                now.set(now.get() + PAUSE);
            },
        );
        assert_eq!(found, Ok("leased"));
        assert_eq!(paused.get(), 2);
    }

    #[test]
    fn each_send_waits_five_seconds_or_the_time_left() {
        let waits = RefCell::new(Vec::new());
        let now = Cell::new(Duration::ZERO);
        let result = poll(
            |wait| {
                waits.borrow_mut().push(wait);
                now.set(now.get() + Duration::from_secs(4));
                Ok(reply("down"))
            },
            "net config",
            Duration::from_secs(10),
            |_| None::<()>,
            || now.get(),
            || {},
        );
        assert!(matches!(result, Err(Error::NotReached { .. })));
        assert_eq!(
            *waits.borrow(),
            [
                Duration::from_secs(5),
                Duration::from_secs(5),
                Duration::from_secs(2)
            ]
        );
    }

    #[test]
    fn nothing_found_in_time_names_the_line_the_wait_and_the_last_reply() {
        let sent = Cell::new(0);
        let now = Cell::new(Duration::ZERO);
        let result = poll(
            |_| {
                sent.set(sent.get() + 1);
                // A wait that never ends would send for ever. Fail instead of hanging.
                assert!(sent.get() < 100, "the wait did not stop at the deadline");
                now.set(now.get() + Duration::from_secs(1));
                Ok(reply(&format!("down {}", sent.get())))
            },
            "net config",
            Duration::from_secs(3),
            |_| None::<()>,
            || now.get(),
            || {},
        );
        let error = result.unwrap_err();
        assert_eq!(
            error,
            Error::NotReached {
                line: "net config".to_owned(),
                waited: Duration::from_secs(3),
                last: Some(reply("down 3")),
            }
        );
        assert_eq!(
            error.to_string(),
            "net config did not give the awaited reply within 3s. The last reply:\n  link: down 3\n  ok"
        );
    }

    #[test]
    fn an_err_reply_goes_to_found_and_a_failed_send_ends_the_wait_at_once() {
        let err = Reply {
            body: vec![],
            result: CommandResult::Err("no link".to_owned()),
        };
        let found = poll(
            |_| Ok(err.clone()),
            "net config",
            Duration::from_secs(20),
            |reply| (reply.result != CommandResult::Ok).then_some(()),
            || Duration::ZERO,
            || panic!("paused after a reply that found gave a value for"),
        );
        assert_eq!(found, Ok(()));
        let failed = poll(
            |_| Err(Error::Run(mcu_run::Error::Transport("gone".to_owned()))),
            "net config",
            Duration::from_secs(20),
            |_| Some(()),
            || Duration::ZERO,
            || panic!("paused after a failed send"),
        );
        assert_eq!(
            failed,
            Err(Error::Run(mcu_run::Error::Transport("gone".to_owned())))
        );
    }
}
