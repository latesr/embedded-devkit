//! The console client over an in-memory transport that replays what a board sends (M-12).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use mcu_run::{Banner, Client, CommandResult, Error, Reply};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::Duration;

/// A transport that gives the board's output in the chunks it was scripted in, one chunk per read, and times
/// out once they are gone, one millisecond per read. It records what the client sent. A client that ignored
/// its deadline would read for two seconds and then fail on the transport instead of the timeout.
struct Board {
    chunks: VecDeque<Vec<u8>>,
    sent: Vec<u8>,
    timeouts: u32,
}

impl Board {
    fn says(chunks: &[&str]) -> Self {
        Self {
            chunks: chunks
                .iter()
                .map(|chunk| chunk.as_bytes().to_vec())
                .collect(),
            sent: Vec::new(),
            timeouts: 0,
        }
    }
}

impl Read for Board {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.chunks.pop_front() {
            Some(chunk) => {
                buf[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }
            None => {
                self.timeouts += 1;
                if self.timeouts > 2000 {
                    return Err(io::Error::other("the client read past the deadline"));
                }
                std::thread::sleep(Duration::from_millis(1));
                Err(io::Error::new(io::ErrorKind::TimedOut, "nothing arrived"))
            }
        }
    }
}

impl Write for Board {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sent.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const TIMEOUT: Duration = Duration::from_millis(200);

#[test]
fn a_reply_is_the_body_lines_and_the_result_line_and_the_line_is_sent_with_cr() {
    let mut client = Client::new(Board::says(&["echo hi  there\r\nhi  there\r\nok\r\nc5> "]));
    let reply = client.send("echo hi  there", TIMEOUT).unwrap();
    assert_eq!(
        reply,
        Reply {
            body: vec!["hi  there".to_owned()],
            result: CommandResult::Ok,
        }
    );
    assert_eq!(client.into_inner().sent, b"echo hi  there\r");
}

#[test]
fn an_err_result_line_has_its_reason_and_the_body_can_be_empty() {
    let mut client = Client::new(Board::says(&["frob\r\nerr unknown command frob\r\nc5> "]));
    let reply = client.send("frob", TIMEOUT).unwrap();
    assert_eq!(
        reply,
        Reply {
            body: vec![],
            result: CommandResult::Err("unknown command frob".to_owned()),
        }
    );
}

#[test]
fn output_split_at_any_point_gives_the_same_reply() {
    let full = "c5> info\r\nboard: c5\r\nchip: esp32c5\r\nok\r\nc5> ";
    for at in 1..full.len() {
        let (first, second) = full.split_at(at);
        let mut client = Client::new(Board::says(&[first, second]));
        let reply = client.send("info", TIMEOUT).unwrap();
        assert_eq!(
            reply,
            Reply {
                body: vec!["board: c5".to_owned(), "chip: esp32c5".to_owned()],
                result: CommandResult::Ok,
            },
            "split at {at}"
        );
    }
}

#[test]
fn the_prompt_left_by_the_last_reply_and_a_masked_echo_are_skipped() {
    let mut client = Client::new(Board::says(&[
        "c5> join home ***\r\nssid: home\r\npassword: p w\r\nok\r\nc5> ",
    ]));
    let reply = client.send("join home p w", TIMEOUT).unwrap();
    assert_eq!(reply.body, ["ssid: home", "password: p w"]);
    assert_eq!(reply.result, CommandResult::Ok);
}

#[test]
fn lines_before_the_echo_are_not_part_of_the_reply() {
    let mut client = Client::new(Board::says(&[
        "info  stale body\r\nok\r\nc5> echo a\r\na\r\nok\r\nc5> ",
    ]));
    let reply = client.send("echo a", TIMEOUT).unwrap();
    assert_eq!(reply.body, ["a"]);
}

#[test]
fn no_result_line_within_the_timeout_is_an_error_that_names_what_was_waited_for() {
    let mut client = Client::new(Board::says(&["echo a\r\na\r\npartial"]));
    let error = client.send("echo a", TIMEOUT).unwrap_err();
    assert_eq!(
        error,
        Error::Timeout {
            waiting_for: "the result line".to_owned(),
            partial: "partial".to_owned(),
        }
    );
    assert_eq!(
        error.to_string(),
        "the result line did not arrive within the timeout. Unfinished line: partial"
    );
}

#[test]
fn a_field_is_the_value_of_the_first_line_with_its_whole_key() {
    let reply = Reply {
        body: [
            "boardx: other",
            "board: c5",
            "chip: esp32c5 v1.0",
            "board: second",
            "empty: ",
            "bare:",
        ]
        .map(str::to_owned)
        .to_vec(),
        result: CommandResult::Ok,
    };
    assert_eq!(reply.field("board"), Some("c5"));
    assert_eq!(reply.field("chip"), Some("esp32c5 v1.0"));
    assert_eq!(reply.field("empty"), Some(""));
    assert_eq!(reply.field("bare"), None);
    assert_eq!(reply.field("boa"), None);
    assert_eq!(reply.field("missing"), None);
}

#[test]
fn a_protocol_error_names_both_versions_or_the_missing_one() {
    let other = Error::Protocol {
        firmware: Some(2),
        host: 1,
    };
    assert_eq!(
        other.to_string(),
        "the firmware speaks console protocol 2, and this mcu-run speaks 1. Build the firmware's mcu-console \
         and the host's mcu-run from one release"
    );
    let none = Error::Protocol {
        firmware: None,
        host: 1,
    };
    assert_eq!(
        none.to_string(),
        "the firmware's banner names no console protocol, so its mcu-console is older than protocol 1, which \
         this mcu-run speaks"
    );
}

#[test]
fn the_banner_is_found_after_the_boot_noise_and_the_next_command_works() {
    let mut client = Client::new(Board::says(&[
        "ESP-ROM:esp32c5-20230419\r\nBuild:Apr 19 2023\r\nrst:0x1 (POWERON),boot:0x1c\r\n",
        "\r\nc5 boot 2 protocol 1\r\nc5> ",
        "echo a\r\na\r\nok\r\nc5> ",
    ]));
    assert_eq!(
        client.wait_for_banner(TIMEOUT).unwrap(),
        Banner {
            name: "c5".to_owned(),
            boot: 2,
            protocol: Some(1),
            image: None,
        }
    );
    assert_eq!(client.send("echo a", TIMEOUT).unwrap().body, ["a"]);
}

#[test]
fn no_banner_within_the_timeout_is_an_error() {
    let mut client = Client::new(Board::says(&["noise\r\nmore noise\r\n"]));
    let error = client.wait_for_banner(TIMEOUT).unwrap_err();
    assert!(
        matches!(&error, Error::Timeout { waiting_for, .. } if waiting_for == "the banner"),
        "{error:?}"
    );
}

#[test]
fn a_board_that_never_stops_talking_still_hits_the_deadline() {
    /// A line every millisecond. A client that ignored the deadline would read for two seconds and then fail
    /// on the transport instead of the timeout.
    struct Chatter(u32);
    impl Read for Chatter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0 += 1;
            if self.0 > 2000 {
                return Err(io::Error::other("the client read past the deadline"));
            }
            std::thread::sleep(Duration::from_millis(1));
            buf[..6].copy_from_slice(b"noise\n");
            Ok(6)
        }
    }
    impl Write for Chatter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut client = Client::new(Chatter(0));
    assert!(matches!(
        client.send("echo a", TIMEOUT),
        Err(Error::Timeout { .. })
    ));
    let mut client = Client::new(Chatter(0));
    assert!(matches!(
        client.wait_for_banner(TIMEOUT),
        Err(Error::Timeout { .. })
    ));
}

#[test]
fn a_transport_error_other_than_a_timeout_is_reported() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("cable gone"))
        }
    }
    impl Write for Broken {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = Client::new(Broken).send("echo a", TIMEOUT).unwrap_err();
    assert_eq!(error, Error::Transport("cable gone".to_owned()));
}

#[test]
fn a_closed_transport_is_reported() {
    struct Closed;
    impl Read for Closed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }
    impl Write for Closed {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = Client::new(Closed).send("echo a", TIMEOUT).unwrap_err();
    assert_eq!(
        error,
        Error::Transport("the console closed the transport".to_owned())
    );
}

/// A board running `wait`: it echoes the line, then says nothing until it has read `needed` Ctrl-Cs, and then
/// writes `answer`. Each quiet read times out after a millisecond. As with [`Board`], a client that ignored
/// its deadline would fail on the transport after two seconds instead of the timeout.
struct Waiting {
    out: VecDeque<u8>,
    sent: Vec<u8>,
    answer: &'static str,
    needed: usize,
    timeouts: u32,
}

impl Waiting {
    fn new(answer: &'static str, needed: usize) -> Self {
        Self {
            out: b"wait\r\n".iter().copied().collect(),
            sent: Vec::new(),
            answer,
            needed,
            timeouts: 0,
        }
    }
}

impl Read for Waiting {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.out.len().min(buf.len());
        if n == 0 {
            self.timeouts += 1;
            if self.timeouts > 2000 {
                return Err(io::Error::other("the client read past the deadline"));
            }
            std::thread::sleep(Duration::from_millis(1));
            return Err(io::Error::new(io::ErrorKind::TimedOut, "nothing arrived"));
        }
        for slot in &mut buf[..n] {
            *slot = self.out.pop_front().unwrap();
        }
        Ok(n)
    }
}

impl Write for Waiting {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sent.extend_from_slice(bytes);
        let ctrl_cs = self.sent.iter().filter(|byte| **byte == 0x03).count();
        if bytes.contains(&0x03) && ctrl_cs == self.needed {
            self.out.extend(self.answer.bytes());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn ctrl_c_goes_once_the_wait_has_passed_and_the_reply_is_read_after_it() {
    let mut client = Client::new(Waiting::new("stopped\r\nok\r\nc5> ", 1));
    let reply = client
        .send_interrupted("wait", Duration::from_millis(20), TIMEOUT)
        .unwrap();
    assert_eq!(
        reply,
        Reply {
            body: vec!["stopped".to_owned()],
            result: CommandResult::Ok,
        }
    );
    assert_eq!(client.into_inner().sent, b"wait\r\x03");
}

#[test]
fn ctrl_c_is_not_sent_when_the_reply_comes_first() {
    let mut client = Client::new(Board::says(&["echo a\r\na\r\nok\r\nc5> "]));
    let reply = client
        .send_interrupted("echo a", Duration::from_millis(50), TIMEOUT)
        .unwrap();
    assert_eq!(reply.result, CommandResult::Ok);
    assert_eq!(client.into_inner().sent, b"echo a\r");
}

#[test]
fn a_second_ctrl_c_goes_after_twice_the_wait_and_drops_the_command() {
    let mut client = Client::new(Waiting::new("err interrupted\r\nc5> ", 2));
    let reply = client
        .send_interrupted("wait", Duration::from_millis(20), TIMEOUT)
        .unwrap();
    assert_eq!(reply.result, CommandResult::Err("interrupted".to_owned()));
    assert_eq!(client.into_inner().sent, b"wait\r\x03\x03");
}

#[test]
fn a_ctrl_c_due_after_the_timeout_is_not_sent() {
    let mut client = Client::new(Waiting::new("stopped\r\nok\r\nc5> ", 1));
    let error = client
        .send_interrupted("wait", Duration::from_secs(10), TIMEOUT)
        .unwrap_err();
    assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
    assert_eq!(client.into_inner().sent, b"wait\r");
}

#[test]
fn a_board_that_answers_neither_ctrl_c_still_hits_the_timeout() {
    let mut client = Client::new(Waiting::new("", 3));
    let error = client
        .send_interrupted("wait", Duration::from_millis(20), TIMEOUT)
        .unwrap_err();
    assert!(matches!(error, Error::Timeout { .. }), "{error:?}");
}
