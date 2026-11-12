//! What ends a run, and where the board's output goes.

use crate::Error;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// A line of the board's output contained the string the run waited for.
    Matched,
    /// The deadline passed.
    Deadline,
    /// The firmware exited through semihosting with this code.
    Exited(u8),
    /// The core halted for a reason other than a semihosting exit, such as a breakpoint or a fault. The text
    /// is probe-rs's description of the halt.
    Halted(String),
}

impl Stop {
    /// The process exit code for this stop: 0 for a match, 124 at the deadline, the firmware's own code for a
    /// semihosting exit, and 1 for any other halt.
    pub fn code(&self) -> u8 {
        match self {
            Stop::Matched => 0,
            Stop::Deadline => 124,
            Stop::Exited(code) => *code,
            Stop::Halted(_) => 1,
        }
    }
}

/// Takes the board's output a line at a time, writes it to the screen and the log, and says when the run is
/// over.
pub(crate) struct Monitor<W> {
    out: W,
    until: Option<String>,
    deadline: Option<Instant>,
    log: Option<(PathBuf, File)>,
    /// Bytes of a text line that has not ended yet.
    partial: Vec<u8>,
}

impl<W: Write> Monitor<W> {
    /// A monitor that writes to `out`, stops at a line containing `until` or at `timeout` after `now`, and
    /// keeps a copy of the output in `log`.
    pub(crate) fn new(
        out: W,
        until: Option<&str>,
        timeout: Option<Duration>,
        log: Option<&Path>,
        now: Instant,
    ) -> Result<Self, Error> {
        let log = match log {
            Some(path) => Some((
                path.to_path_buf(),
                File::create(path).map_err(|error| Error::Log {
                    path: path.to_path_buf(),
                    error: error.to_string(),
                })?,
            )),
            None => None,
        };
        Ok(Self {
            out,
            until: until.map(str::to_owned),
            deadline: timeout.and_then(|timeout| now.checked_add(timeout)),
            log,
            partial: Vec::new(),
        })
    }

    /// One complete line of output. Returns [`Stop::Matched`] when the line contains the string the run waits
    /// for.
    pub(crate) fn line(&mut self, text: &str) -> Result<Option<Stop>, Error> {
        writeln!(self.out, "{text}").map_err(|error| Error::Output(error.to_string()))?;
        if let Some((path, file)) = &mut self.log {
            writeln!(file, "{text}").map_err(|error| Error::Log {
                path: path.clone(),
                error: error.to_string(),
            })?;
        }
        let matched = self
            .until
            .as_deref()
            .is_some_and(|until| text.contains(until));
        Ok(matched.then_some(Stop::Matched))
    }

    /// Raw bytes of a text channel. Each complete line, with its CR and LF removed, goes through
    /// [`line`](Self::line). A line that has not ended yet waits for the rest.
    pub(crate) fn text(&mut self, bytes: &[u8]) -> Result<Option<Stop>, Error> {
        let mut stop = None;
        for &byte in bytes {
            if byte == b'\n' {
                let raw = std::mem::take(&mut self.partial);
                let line =
                    String::from_utf8_lossy(raw.strip_suffix(b"\r").unwrap_or(&raw)).into_owned();
                if let Some(found) = self.line(&line)? {
                    stop = Some(found);
                }
            } else {
                self.partial.push(byte);
            }
        }
        Ok(stop)
    }

    /// Writes a text line that has not ended, at the end of the run.
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        if !self.partial.is_empty() {
            let raw = std::mem::take(&mut self.partial);
            self.line(&String::from_utf8_lossy(&raw))?;
        }
        Ok(())
    }

    /// Returns [`Stop::Deadline`] once `now` has reached the deadline.
    pub(crate) fn tick(&self, now: Instant) -> Option<Stop> {
        self.deadline
            .is_some_and(|deadline| now >= deadline)
            .then_some(Stop::Deadline)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    fn monitor(until: Option<&str>, timeout: Option<Duration>) -> (Monitor<Vec<u8>>, Instant) {
        let now = Instant::now();
        (
            Monitor::new(Vec::new(), until, timeout, None, now).unwrap(),
            now,
        )
    }

    #[test]
    fn lines_are_written_with_a_line_end_and_the_matching_line_stops_the_run() {
        let (mut monitor, _) = monitor(Some("done"), None);
        assert_eq!(monitor.line("starting").unwrap(), None);
        assert_eq!(monitor.line("all done here").unwrap(), Some(Stop::Matched));
        assert_eq!(monitor.out, b"starting\nall done here\n");
    }

    #[test]
    fn without_a_string_to_wait_for_no_line_stops_the_run() {
        let (mut monitor, _) = monitor(None, None);
        assert_eq!(monitor.line("done").unwrap(), None);
    }

    #[test]
    fn the_deadline_stops_the_run_once_reached() {
        let (monitor, now) = monitor(None, Some(Duration::from_secs(10)));
        assert_eq!(monitor.tick(now), None);
        assert_eq!(monitor.tick(now + Duration::from_secs(9)), None);
        assert_eq!(
            monitor.tick(now + Duration::from_secs(10)),
            Some(Stop::Deadline)
        );
        let (monitor, now) = super::tests::monitor(None, None);
        assert_eq!(monitor.tick(now + Duration::from_secs(3600)), None);
    }

    #[test]
    fn text_bytes_become_lines_at_lf_with_cr_removed_and_a_partial_line_waits() {
        let (mut monitor, _) = monitor(Some("ok"), None);
        assert_eq!(monitor.text(b"mcu> in").unwrap(), None);
        assert_eq!(monitor.out, b"");
        assert_eq!(
            monitor.text(b"fo\r\nboard: c5\r\nok\r\nmcu> ").unwrap(),
            Some(Stop::Matched)
        );
        assert_eq!(monitor.out, b"mcu> info\nboard: c5\nok\n");
        monitor.finish().unwrap();
        assert_eq!(monitor.out, b"mcu> info\nboard: c5\nok\nmcu> \n");
        monitor.finish().unwrap();
        assert_eq!(monitor.out, b"mcu> info\nboard: c5\nok\nmcu> \n");
    }

    #[test]
    fn the_log_holds_every_line() {
        let path = std::env::temp_dir().join(format!("mcu-run-monitor-{}.log", std::process::id()));
        let mut monitor =
            Monitor::new(Vec::new(), None, None, Some(&path), Instant::now()).unwrap();
        monitor.line("one").unwrap();
        monitor.text(b"two\r\nthr").unwrap();
        monitor.finish().unwrap();
        drop(monitor);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nthr\n");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_log_that_cannot_be_created_is_reported_with_its_path() {
        let path = Path::new("/nonexistent-folder/run.log");
        let error = Monitor::new(Vec::new(), None, None, Some(path), Instant::now())
            .err()
            .unwrap();
        assert!(
            matches!(&error, Error::Log { path: found, .. } if found == path),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with("log /nonexistent-folder/run.log:"),
            "{error}"
        );
    }

    #[test]
    fn each_stop_has_its_exit_code() {
        assert_eq!(Stop::Matched.code(), 0);
        assert_eq!(Stop::Deadline.code(), 124);
        assert_eq!(Stop::Exited(0).code(), 0);
        assert_eq!(Stop::Exited(3).code(), 3);
        assert_eq!(Stop::Halted("breakpoint".to_owned()).code(), 1);
    }
}
