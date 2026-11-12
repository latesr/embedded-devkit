//! The host's side of the console: sends a line and reads the body and the result line.

use crate::Error;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

/// The result line of one reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// The command ran.
    Ok,
    /// The command failed with this reason.
    Err(String),
}

/// One reply from the console: the body lines, then the result line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The body lines, without the echo, the result line or the prompt.
    pub body: Vec<String>,
    /// The result line.
    pub result: CommandResult,
}

impl Reply {
    /// The value of the first body line `key: value` for `key`, or `None` when no body line has that key.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.body
            .iter()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
    }
}

/// A console client over any transport whose reads time out, such as a serial port opened with
/// [`open_port`](crate::open_port).
///
/// The console echoes each byte, so the client discards what it reads up to the echo of the line it sent, and
/// the prompt that follows the result line waits on the transport until the next command reads past it.
pub struct Client<T> {
    transport: T,
    line: Vec<u8>,
}

impl<T: Read + Write> Client<T> {
    /// A client over `transport`. A read on the transport must return within a bounded time, with an error of
    /// kind `TimedOut` or `WouldBlock` when nothing arrived, so the client can keep its deadlines.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            line: Vec::new(),
        }
    }

    /// The transport, to close it or hand it on.
    pub fn into_inner(self) -> T {
        self.transport
    }

    /// Sends `command` as one line and returns its reply. Fails with [`Error::Timeout`] when no result line arrives
    /// within `timeout`.
    pub fn send(&mut self, command: &str, timeout: Duration) -> Result<Reply, Error> {
        self.exchange(command, None, timeout)
    }

    /// Sends `command` as one line, sends Ctrl-C once `after` has passed without a result line, and returns the
    /// reply. A command that stops on Ctrl-C writes its summary and its result line. A second Ctrl-C goes at twice
    /// `after`, which drops a command that did not stop, with `err interrupted`. Fails with [`Error::Timeout`]
    /// when no result line arrives within `timeout` of sending the line.
    pub fn send_interrupted(
        &mut self,
        command: &str,
        after: Duration,
        timeout: Duration,
    ) -> Result<Reply, Error> {
        self.exchange(command, Some(after), timeout)
    }

    fn exchange(
        &mut self,
        command: &str,
        interrupt_after: Option<Duration>,
        timeout: Duration,
    ) -> Result<Reply, Error> {
        self.write(format!("{command}\r").as_bytes())?;
        let start = Instant::now();
        let deadline = start.checked_add(timeout);
        let mut interrupts: VecDeque<Instant> = interrupt_after
            .into_iter()
            .flat_map(|after| [after, after.saturating_mul(2)])
            .filter_map(|after| start.checked_add(after))
            .collect();
        let mut echoed = false;
        let mut body = Vec::new();
        loop {
            let line = match interrupts.front() {
                None => self.read_line(deadline, "the result line")?,
                Some(at) => {
                    let until = deadline.map_or(*at, |deadline| deadline.min(*at));
                    match self.read_line(Some(until), "the result line") {
                        Err(Error::Timeout { .. }) => {
                            self.check_deadline(deadline, "the result line")?;
                            self.write(&[0x03])?;
                            interrupts.pop_front();
                            continue;
                        }
                        line => line?,
                    }
                }
            };
            self.check_deadline(deadline, "the result line")?;
            if !echoed {
                echoed = is_echo(&line, command);
                continue;
            }
            match result_line(&line) {
                Some(result) => return Ok(Reply { body, result }),
                None => body.push(line),
            }
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.transport
            .write_all(bytes)
            .and_then(|()| self.transport.flush())
            .map_err(|error| Error::Transport(error.to_string()))
    }

    /// Discards everything up to the console's banner, `<name> boot <n> protocol <v>`, and returns it. After a reset the ROM
    /// and the bootloader can write to the same transport, and this skips them. Fails with [`Error::Timeout`]
    /// when no banner arrives within `timeout`.
    pub fn wait_for_banner(&mut self, timeout: Duration) -> Result<Banner, Error> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let line = self.read_line(deadline, "the banner")?;
            if let Some(banner) = banner(&line) {
                return Ok(banner);
            }
            self.check_deadline(deadline, "the banner")?;
        }
    }

    /// Fails with [`Error::Timeout`] once `deadline` has passed. A board that writes lines without end would
    /// otherwise keep a wait going for ever.
    fn check_deadline(&self, deadline: Option<Instant>, waiting_for: &str) -> Result<(), Error> {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(Error::Timeout {
                waiting_for: waiting_for.to_owned(),
                partial: String::from_utf8_lossy(&self.line).into_owned(),
            });
        }
        Ok(())
    }

    /// The next complete line, without its CR and LF. Bytes read past the line stay for the next call.
    fn read_line(&mut self, deadline: Option<Instant>, waiting_for: &str) -> Result<String, Error> {
        let mut buf = [0u8; 256];
        loop {
            if let Some(at) = self.line.iter().position(|byte| *byte == b'\n') {
                let rest = self.line.split_off(at.saturating_add(1));
                let raw = std::mem::replace(&mut self.line, rest);
                let raw = raw
                    .strip_suffix(b"\r\n")
                    .or(raw.strip_suffix(b"\n"))
                    .unwrap_or(&raw);
                return Ok(String::from_utf8_lossy(raw).into_owned());
            }
            self.check_deadline(deadline, waiting_for)?;
            match self.transport.read(&mut buf) {
                Ok(0) => {
                    return Err(Error::Transport(
                        "the console closed the transport".to_owned(),
                    ));
                }
                Ok(n) => self.line.extend_from_slice(buf.get(..n).unwrap_or(&[])),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(Error::Transport(error.to_string())),
            }
        }
    }
}

/// Whether `line` is the echo of `command`: it ends with the command, with any suffix of it replaced by `*`
/// where the console hid a secret argument. A prompt left unread sits before the echo on the same line.
fn is_echo(line: &str, command: &str) -> bool {
    let Some(tail) = line
        .len()
        .checked_sub(command.len())
        .and_then(|at| line.get(at..))
    else {
        return false;
    };
    let same = tail
        .bytes()
        .zip(command.bytes())
        .take_while(|(found, wanted)| found == wanted)
        .count();
    tail.bytes().skip(same).all(|byte| byte == b'*')
}

fn result_line(line: &str) -> Option<CommandResult> {
    if line == "ok" {
        return Some(CommandResult::Ok);
    }
    line.strip_prefix("err ")
        .map(|reason| CommandResult::Err(reason.to_owned()))
}

/// The banner line a console writes when it starts, `<name> boot <n> protocol <v>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    /// The application's name, which also opens the prompt.
    pub name: String,
    /// How many times the application has booted since power-on.
    pub boot: u32,
    /// The console's protocol version, or `None` for a console from before the banner named it.
    pub protocol: Option<u32>,
    /// The ID of the running image in lower-case hex, such as its GNU build ID, or `None` when the banner names
    /// none.
    pub image: Option<String>,
}

/// The banner in `line`, when the line is one.
fn banner(line: &str) -> Option<Banner> {
    let (name, rest) = line.rsplit_once(" boot ")?;
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    let mut words = rest.split_whitespace();
    let boot = words.next()?.parse().ok()?;
    let protocol = match (words.next(), words.next()) {
        (None, _) => None,
        (Some("protocol"), Some(version)) => Some(version.parse().ok()?),
        _ => return None,
    };
    let image = match (words.next(), words.next(), words.next()) {
        (None, _, _) => None,
        (Some("image"), Some(id), None) if is_image_id(id) => Some(id.to_owned()),
        _ => return None,
    };
    Some(Banner {
        name: name.to_owned(),
        boot,
        protocol,
        image,
    })
}

/// Whether `id` is whole bytes in lower-case hex, as the console writes an image ID.
fn is_image_id(id: &str) -> bool {
    id.len().is_multiple_of(2)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Fails with [`Error::Protocol`] when `banner` names another version than [`PROTOCOL`](crate::PROTOCOL), or none.
pub(crate) fn check_protocol(banner: &Banner) -> Result<(), Error> {
    if banner.protocol == Some(crate::PROTOCOL) {
        return Ok(());
    }
    Err(Error::Protocol {
        firmware: banner.protocol,
        host: crate::PROTOCOL,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_echo_ends_with_the_command_and_may_have_a_prompt_before_it() {
        assert!(is_echo("echo hi", "echo hi"));
        assert!(is_echo("c5> echo hi", "echo hi"));
        assert!(is_echo("join home ***", "join home p w"));
        assert!(!is_echo("hi", "echo hi"));
        assert!(!is_echo("echo hi there", "echo hi"));
        assert!(!is_echo("", "echo"));
    }

    #[test]
    fn the_result_line_is_ok_or_err_with_a_reason() {
        assert_eq!(result_line("ok"), Some(CommandResult::Ok));
        assert_eq!(
            result_line("err unknown command frob"),
            Some(CommandResult::Err("unknown command frob".to_owned()))
        );
        assert_eq!(result_line("okay"), None);
        assert_eq!(result_line("error"), None);
        assert_eq!(result_line("board: c5"), None);
    }

    #[test]
    fn the_banner_gives_the_name_the_boot_count_and_the_protocol() {
        let found = |name: &str, boot: u32, protocol: Option<u32>| {
            Some(Banner {
                name: name.to_owned(),
                boot,
                protocol,
                image: None,
            })
        };
        assert_eq!(banner("c5 boot 3 protocol 1"), found("c5", 3, Some(1)));
        assert_eq!(banner("mcu boot 10 protocol 2"), found("mcu", 10, Some(2)));
        assert_eq!(banner("c5 boot 3"), found("c5", 3, None));
        assert_eq!(banner("ESP-ROM:esp32c5-20230419"), None);
        assert_eq!(banner("c5 boot x"), None);
        assert_eq!(banner("c5 boot 3 protocol x"), None);
        assert_eq!(banner("c5 boot 3 protocol"), None);
        assert_eq!(banner("c5 boot 3 version 1"), None);
        assert_eq!(banner("c5 boot 3 protocol 1 extra"), None);
        assert_eq!(banner("c5> echo boot 3"), None);
        assert_eq!(banner(" boot 3"), None);
    }

    #[test]
    fn the_banner_gives_the_image_as_even_lower_case_hex_or_is_no_banner() {
        assert_eq!(
            banner("c5 boot 3 protocol 1 image 001fabc0"),
            Some(Banner {
                name: "c5".to_owned(),
                boot: 3,
                protocol: Some(1),
                image: Some("001fabc0".to_owned()),
            })
        );
        for line in [
            "c5 boot 3 protocol 1 image",
            "c5 boot 3 protocol 1 image 001FABC0",
            "c5 boot 3 protocol 1 image 001fab0",
            "c5 boot 3 protocol 1 image 00xy",
            "c5 boot 3 protocol 1 image 00 extra",
            "c5 boot 3 protocol 1 name 0011",
        ] {
            assert_eq!(banner(line), None, "{line}");
        }
    }

    #[test]
    fn only_a_banner_with_this_protocol_is_accepted() {
        let with = |protocol| Banner {
            name: "c5".to_owned(),
            boot: 1,
            protocol,
            image: None,
        };
        assert_eq!(check_protocol(&with(Some(crate::PROTOCOL))), Ok(()));
        assert_eq!(
            check_protocol(&with(Some(crate::PROTOCOL + 1))),
            Err(Error::Protocol {
                firmware: Some(crate::PROTOCOL + 1),
                host: crate::PROTOCOL,
            })
        );
        assert_eq!(
            check_protocol(&with(None)),
            Err(Error::Protocol {
                firmware: None,
                host: crate::PROTOCOL,
            })
        );
    }
}
