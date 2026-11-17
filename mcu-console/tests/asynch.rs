//! The async driver over in-memory `embedded-io-async` fakes (K-3, K-5, K-7, K-9, K-11, K-14, K-15, K-19,
//! K-20, K-21, K-22, K-23).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::{CONFIG, Delay, Wifi, long_line_output};
use embedded_io_async::{ErrorType, Read, Write};
use mcu_console::asynch::Console;
use mcu_console::{Config, Stop};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

/// Polls `future` until it completes, and fails a test whose future does not. The fakes complete at once, or
/// never, and a write that never completes loses to the delay.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("the console did not finish");
}

/// Bytes the host sent. Empty means the host closed the transport. `0xfe` is a read error. `0xfd` is a pause: the
/// read stays pending once there, as it does while the host sends nothing, so a running command is polled. `0xfc`
/// marks the end of the input once a read has returned it. A read there fails with an error that the console does
/// not expect. A console that does not stop at the end of the input then fails a test, and does not hang.
struct Rx(VecDeque<u8>);

impl ErrorType for Rx {
    type Error = embedded_io::ErrorKind;
}

impl Read for Rx {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if self.0.front() == Some(&0xfd) {
            self.0.pop_front();
            embassy_futures::yield_now().await;
        }
        if self.0.front() == Some(&0xfe) {
            return Err(embedded_io::ErrorKind::Other);
        }
        if self.0.front() == Some(&0xfc) {
            return Err(embedded_io::ErrorKind::BrokenPipe);
        }
        // One byte per read, so a line split at any point still works.
        match self.0.pop_front() {
            Some(byte) => {
                buf[0] = byte;
                Ok(1)
            }
            None => {
                self.0.push_back(0xfc);
                Ok(0)
            }
        }
    }
}

/// What the console wrote, and whether the host is reading. A host that is not reading never completes a write,
/// as USB Serial/JTAG does.
#[derive(Default)]
struct Host {
    out: Vec<u8>,
    ready: bool,
    flushes: usize,
}

struct Tx(Rc<RefCell<Host>>);

impl ErrorType for Tx {
    type Error = embedded_io::ErrorKind;
}

impl Write for Tx {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if !self.0.borrow().ready {
            std::future::pending::<()>().await;
        }
        self.0.borrow_mut().out.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        if !self.0.borrow().ready {
            std::future::pending::<()>().await;
        }
        self.0.borrow_mut().flushes += 1;
        Ok(())
    }
}

/// Runs the console over `input` until the input ends, and gives what it wrote.
fn session(input: &[u8]) -> (Stop<embedded_io::ErrorKind>, String, u32, usize) {
    session_with(input, CONFIG)
}

/// Runs the console with `config` over `input` until the input ends, and gives what it wrote.
fn session_with(
    input: &[u8],
    config: Config,
) -> (Stop<embedded_io::ErrorKind>, String, u32, usize) {
    let host = Rc::new(RefCell::new(Host {
        ready: true,
        ..Host::default()
    }));
    let waited = Rc::new(RefCell::new(0));
    let mut console: Console<_, _, _, _, 16> = Console::new(
        Rx(input.iter().copied().collect()),
        Tx(host.clone()),
        Delay(waited.clone()),
        Wifi,
        config,
    );
    let stop = block_on(console.run());
    let host = host.borrow();
    let out = String::from_utf8_lossy(&host.out).into_owned();
    (stop, out, *waited.borrow(), host.flushes)
}

#[test]
fn the_banner_the_echo_the_body_the_result_line_and_the_prompt_are_exact() {
    let (stop, out, waited, _) = session(b"echo hi  there\r");
    assert_eq!(stop, Stop::Eof);
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> echo hi  there\r\nhi  there\r\nok\r\nmcu> "
    );
    assert_eq!(waited, 0);
}

#[test]
fn the_banner_names_the_image_in_lower_case_hex_when_the_application_gives_one() {
    let config = Config {
        image: Some(&[0x00, 0x1f, 0xab, 0xc0]),
        ..CONFIG
    };
    let (_, out, _, _) = session_with(b"", config);
    assert_eq!(out, "\r\nmcu boot 3 protocol 1 image 001fabc0\r\nmcu> ");
}

#[test]
fn an_empty_line_gives_the_prompt_alone() {
    let (_, out, _, _) = session(b"\r\n\n");
    assert_eq!(out, "\r\nmcu boot 3 protocol 1\r\nmcu> \r\nmcu> \r\nmcu> ");
}

#[test]
fn backspace_erases_on_the_terminal_and_ctrl_c_prompts_again() {
    let (_, out, _, _) = session(b"ex\x7fcho a\x03echo b\r");
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> ex\x08 \x08cho a\r\nmcu> echo b\r\nb\r\nok\r\nmcu> "
    );
}

#[test]
fn a_secret_argument_echoes_as_stars_and_reaches_the_command() {
    let (_, out, _, _) = session(b"join home p w\r");
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> join home ***\r\nssid: home\r\npassword: p w\r\nok\r\nmcu> "
    );
}

#[test]
fn help_and_an_unknown_command_and_a_long_line_are_reported() {
    let (_, out, _, _) = session(b"help\rfrob\r0123456789abcdefg\r");
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> help\r\n\
         help: list the commands\r\nreset: reset the chip\r\n\
         echo <text>: write the text back\r\njoin <ssid> <password>: join a network\r\nok\r\n\
         mcu> frob\r\nerr unknown command frob\r\n\
         mcu> 0123456789abcdef\r\nerr line too long\r\nmcu> "
    );
}

#[test]
fn a_body_line_is_cut_at_the_reply_length() {
    let (_, out, _, _) = session(b"long\r");
    assert_eq!(out, long_line_output());
}

#[test]
fn a_read_error_stops_the_driver() {
    let (stop, out, _, _) = session(b"e\xfe");
    assert_eq!(stop, Stop::Read(embedded_io::ErrorKind::Other));
    assert_eq!(out, "\r\nmcu boot 3 protocol 1\r\nmcu> e");
}

#[test]
fn ctrl_c_asks_a_running_command_to_stop_and_it_writes_its_result() {
    let (stop, out, _, _) = session(b"wait\r\xfd\x03\xfd");
    assert_eq!(stop, Stop::Eof);
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> wait\r\nstopped\r\nok\r\nmcu> "
    );
}

#[test]
fn a_second_ctrl_c_drops_a_command_that_does_not_stop() {
    let (stop, out, _, _) = session(b"hang\r\xfd\x03\xfd\x03echo a\r");
    assert_eq!(stop, Stop::Eof);
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> hang\r\nbusy\r\nerr interrupted\r\nmcu> echo a\r\na\r\nok\r\nmcu> "
    );
}

#[test]
fn other_bytes_sent_while_a_command_runs_are_dropped() {
    let (_, out, _, _) = session(b"wait\r\xfdx\x03\xfdecho a\r");
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> wait\r\nstopped\r\nok\r\nmcu> echo a\r\na\r\nok\r\nmcu> "
    );
}

#[test]
fn a_stop_asked_of_one_command_does_not_reach_the_next() {
    // The second `wait` sees no Ctrl-C, so it runs until the input ends.
    let (stop, out, _, _) = session(b"wait\r\xfd\x03\xfdwait\r");
    assert_eq!(stop, Stop::Eof);
    assert_eq!(
        out,
        "\r\nmcu boot 3 protocol 1\r\nmcu> wait\r\nstopped\r\nok\r\nmcu> wait\r\n"
    );
}

#[test]
fn the_input_ending_or_failing_while_a_command_runs_stops_the_driver() {
    let (stop, out, _, _) = session(b"hang\r\xfd\x03");
    assert_eq!(stop, Stop::Eof);
    assert_eq!(out, "\r\nmcu boot 3 protocol 1\r\nmcu> hang\r\nbusy\r\n");
    let (stop, _, _, _) = session(b"hang\r\xfd\xfe");
    assert_eq!(stop, Stop::Read(embedded_io::ErrorKind::Other));
}

#[test]
fn output_is_flushed_after_the_prompt_and_after_each_read() {
    let (_, _, _, flushes) = session(b"echo a\r");
    // The banner's prompt, then for the 7 input bytes: 6 reads flush once, and the CR flushes the prompt and
    // the read.
    assert_eq!(flushes, 1 + 6 + 2);
}

#[test]
fn a_host_that_does_not_read_costs_one_timeout_and_then_nothing_until_it_types() {
    let host = Rc::new(RefCell::new(Host::default()));
    let waited = Rc::new(RefCell::new(0));
    let mut console: Console<_, _, _, _, 16> = Console::new(
        Rx(b"echo a\r".iter().copied().collect()),
        Tx(host.clone()),
        Delay(waited.clone()),
        Wifi,
        CONFIG,
    );
    block_on(console.start());
    assert_eq!(*waited.borrow(), 500);
    assert!(host.borrow().out.is_empty());
    // The first input byte shows a host, so the console tries once more and waits one more timeout.
    block_on(console.step()).unwrap();
    assert_eq!(*waited.borrow(), 1000);
    assert!(host.borrow().out.is_empty());
    // The host starts reading. The rest of the line is echoed and answered.
    host.borrow_mut().ready = true;
    while block_on(console.step()).is_ok() {}
    assert_eq!(*waited.borrow(), 1000);
    assert_eq!(
        String::from_utf8_lossy(&host.borrow().out),
        "cho a\r\na\r\nok\r\nmcu> "
    );
}

#[test]
fn reset_writes_ok_and_flushes_before_the_reset_function_runs() {
    let host = Rc::new(RefCell::new(Host {
        ready: true,
        ..Host::default()
    }));
    let mut console: Console<_, _, _, _, 16> = Console::new(
        Rx(b"reset\r".iter().copied().collect()),
        Tx(host.clone()),
        Delay::default(),
        Wifi,
        CONFIG,
    );
    block_on(console.start());
    let flushes_before = host.borrow().flushes;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        while block_on(console.step()).is_ok() {}
    }));
    assert!(result.is_err());
    let host = host.borrow();
    assert_eq!(
        String::from_utf8_lossy(&host.out),
        "\r\nmcu boot 3 protocol 1\r\nmcu> reset\r\nok\r\n"
    );
    // The reads of "reset" flushed 5 times, then the reset flushed the ok.
    assert_eq!(host.flushes, flushes_before + 5 + 1);
}
