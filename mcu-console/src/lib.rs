//! A line console for a microcontroller.
//!
//! The host sends a line. The console echoes each byte as it is typed, runs the command the line names, writes
//! zero or more body lines, then one result line, `ok` or `err <reason>`, then the prompt. A person at a
//! terminal and a test runner read the same output.
//!
//! [`Input`] is the core. It takes one byte at a time and reports what the byte did, and it does no I/O. The
//! drivers own the transport and the loop. The application supplies its commands as a type that implements
//! [`Commands`], and composes sets as a tuple.
//!
//! # The line grammar
//!
//! Input:
//!
//! - A line ends at CR, LF or CRLF. A tab counts as a space.
//! - Backspace, `0x08` or `0x7f`, deletes the last byte. Ctrl-C, `0x03`, abandons the line. Every other
//!   control byte is ignored.
//! - The first word names the command. The words after it are its arguments, split on runs of spaces. A
//!   command can take one free-text value, last, which is the rest of the line. There is no quoting.
//! - A line longer than the console's buffer is refused whole with `err line too long`. A line that is not
//!   UTF-8 is refused with `err not utf-8`. An empty line gives the prompt and no result line.
//!
//! Output, for each line:
//!
//! - the echo of the line as it was typed, with each byte of a secret argument echoed as `*`
//! - zero or more body lines, each cut at [`REPLY_LEN`] bytes
//! - one result line, `ok` or `err <reason>`
//! - the prompt, `<name>> `
//!
//! Every output line ends in CRLF. The prompt has no line end.
//!
//! # Stopping a command
//!
//! While a command runs, the async driver reads the host's input. The first Ctrl-C asks the command to stop
//! through its [`Cancel`]. A command that watches for it writes what it has and returns its result. A second
//! Ctrl-C drops the command, and the driver writes `err interrupted`. Every other byte is dropped. The blocking
//! driver does not read while a command runs.
//!
//! # Built-in commands
//!
//! `help` writes one body line per command, the two built-ins first. `reset` writes `ok`, flushes it and resets
//! the chip through the function the application supplies.

#![cfg_attr(not(test), no_std)]
// The console runs on one executor, and its futures are not `Send`, as `embedded-io-async`'s are not.
#![allow(async_fn_in_trait)]

mod args;
pub mod asynch;
pub mod blocking;
mod cancel;
mod input;
mod linebuf;
mod secret;

pub use args::{Args, flag, parse_size};
pub use cancel::{Cancel, CancelSignal};
pub use input::{Action, Input, LineError};

use core::fmt::{Display, Write as _};
use linebuf::LineBuf;

/// The longest body line, in bytes. [`Reply::field`] and the result line cut what does not fit.
pub const REPLY_LEN: usize = 256;

/// The version of the console protocol, which the banner writes as `protocol <v>` (K-9). It rises only when a
/// host must change to keep working, such as for a new line ending or result line. A new command does not raise it.
/// `mcu-run` refuses a console with another version.
pub const PROTOCOL: u32 = 1;

/// What the application tells the console about itself.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The application's name. It opens the banner, and the prompt is `<name>> `.
    pub name: &'static str,
    /// How many times the application has booted. The banner is `<name> boot <boot> protocol <PROTOCOL>`.
    pub boot: u32,
    /// How long one write may wait for the host, in milliseconds. After a write waits this long, the console
    /// drops its output until the next input byte.
    pub write_timeout_ms: u32,
    /// Resets the chip. The `reset` command calls it after writing `ok`.
    pub reset: fn() -> !,
    /// The ID of the running image, such as the GNU build ID that [`gnu_build_id`] reads. The banner ends with
    /// `image <hex>` when it is given, so a host can tell which build answers.
    pub image: Option<&'static [u8]>,
}

/// The GNU build ID in `note`, an ELF note as the linker writes it for `--build-id`, in little-endian order.
///
/// Returns `None` when `note` ends before the note does, or holds a note of another owner or type, or an empty
/// ID. Bytes after the note are ignored.
pub fn gnu_build_id(note: &[u8]) -> Option<&[u8]> {
    const NT_GNU_BUILD_ID: u32 = 3;
    let word = |at: usize| -> Option<u32> {
        let bytes = note.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };
    let (name_size, id_size, kind) = (word(0)?, word(4)?, word(8)?);
    if name_size != 4 || kind != NT_GNU_BUILD_ID || note.get(12..16)? != b"GNU\0" {
        return None;
    }
    let id = note.get(16..16usize.checked_add(usize::try_from(id_size).ok()?)?)?;
    (!id.is_empty()).then_some(id)
}

/// The banner for `config`: `<name> boot <boot> protocol <PROTOCOL>`, then ` image <hex>` when the image is
/// named, between line ends.
fn banner(config: &Config) -> LineBuf<REPLY_LEN> {
    let mut banner = LineBuf::<REPLY_LEN>::new();
    // The boot output before the banner can end in the middle of a line, so the banner starts with a line end.
    let _ = write!(
        banner,
        "\r\n{} boot {} protocol {}",
        config.name, config.boot, PROTOCOL
    );
    if let Some(image) = config.image {
        let _ = write!(banner, " image ");
        for byte in image {
            let _ = write!(banner, "{byte:02x}");
        }
    }
    let _ = write!(banner, "\r\n");
    banner
}

/// Why a driver returned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop<E> {
    /// The transport reported end of input.
    Eof,
    /// The transport failed a read.
    Read(E),
}

/// What a command returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandResult {
    /// The command ran. The console writes `ok`.
    Ok,
    /// The command failed. The console writes `err <reason>`.
    Err(&'static str),
    /// The set has no command of that name. The console tries the next set, then writes
    /// `err unknown command <name>`.
    Unknown,
}

/// One line of `help`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Help {
    /// The command's name and any fixed words, then its placeholders in angle brackets: `join <ssid> <password>`.
    /// Every placeholder comes after every fixed word, because a secret is found by counting the placeholders.
    pub usage: &'static str,
    /// What the command does, in a few words.
    pub summary: &'static str,
    /// The last placeholder is a secret. The console echoes each byte of it as `*`.
    pub secret: bool,
}

/// Where a command writes its body lines. The drivers implement it.
pub trait Reply {
    /// Writes one body line.
    async fn line(&mut self, text: &str);

    /// Writes the bytes written so far to the transport.
    async fn flush(&mut self);

    /// Writes one `key: value` body line, cut at [`REPLY_LEN`] bytes.
    async fn field(&mut self, key: &str, value: impl Display) {
        let mut text = LineBuf::<REPLY_LEN>::new();
        let _ = write!(text, "{key}: {value}");
        self.line(text.as_str()).await;
    }
}

/// A set of commands. The application composes sets as a tuple, `(A, B)`, and the first set that knows a name
/// runs it. `()` is the empty set.
pub trait Commands {
    /// This set's entries for `help`, in the order `help` lists them.
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        core::iter::empty()
    }

    /// Runs `name` with its arguments, or returns [`CommandResult::Unknown`] when this set has no such command.
    ///
    /// `cancel` tells the command when the host asks it to stop. A command that runs until stopped waits on it,
    /// writes its summary and returns its result. The async driver drops a command that does not return after a
    /// second Ctrl-C, so a command keeps nothing at an await that a drop would leave in a bad state.
    async fn run<R: Reply>(
        &mut self,
        name: &str,
        args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult;
}

impl Commands for () {
    async fn run<R: Reply>(
        &mut self,
        _: &str,
        _: Args<'_>,
        _: &mut R,
        _: Cancel<'_>,
    ) -> CommandResult {
        CommandResult::Unknown
    }
}

impl<A: Commands, B: Commands> Commands for (A, B) {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        self.0.help().chain(self.1.help())
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        match self.0.run(name, args, reply, cancel).await {
            CommandResult::Unknown => self.1.run(name, args, reply, cancel).await,
            result => result,
        }
    }
}

const BUILT_IN: [Help; 2] = [
    Help {
        usage: "help",
        summary: "list the commands",
        secret: false,
    },
    Help {
        usage: "reset",
        summary: "reset the chip",
        secret: false,
    },
];

/// Whether the byte just pushed to `input` is part of a secret argument, so the driver echoes `*` for it.
pub(crate) fn masked<C: Commands, const N: usize>(commands: &C, input: &Input<N>) -> bool {
    let line = input.line();
    secret::secret_start(commands, line).is_some_and(|start| line.len() > start)
}

/// Runs one completed line and writes its body and result line. An empty line writes nothing. `reset` is the
/// function the application supplies for the `reset` command.
pub(crate) async fn execute<C: Commands, R: Reply>(
    commands: &mut C,
    line: Result<&str, LineError>,
    reply: &mut R,
    reset: fn() -> !,
    cancel: Cancel<'_>,
) {
    let mut text = LineBuf::<REPLY_LEN>::new();
    let failure: Option<&str> = match line {
        Err(LineError::TooLong) => Some("line too long"),
        Err(LineError::NotUtf8) => Some("not utf-8"),
        Ok(line) => {
            let mut args = Args::new(line);
            match args.next() {
                None => return,
                Some("help") => {
                    for help in BUILT_IN.iter().chain(commands.help()) {
                        reply.field(help.usage, help.summary).await;
                    }
                    None
                }
                Some("reset") => {
                    reply.line("ok").await;
                    reply.flush().await;
                    reset()
                }
                Some(name) => match commands.run(name, args, reply, cancel).await {
                    CommandResult::Ok => None,
                    CommandResult::Err(reason) => Some(reason),
                    CommandResult::Unknown => {
                        let _ = write!(text, "err unknown command {name}");
                        reply.line(text.as_str()).await;
                        return;
                    }
                },
            }
        }
    };
    match failure {
        None => reply.line("ok").await,
        Some(reason) => {
            let _ = write!(text, "err {reason}");
            reply.line(text.as_str()).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    /// Polls `future` until it completes. The fakes never return `Pending`.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
    }

    #[derive(Default)]
    struct Recorder {
        lines: Vec<String>,
    }

    impl Reply for Recorder {
        async fn line(&mut self, text: &str) {
            self.lines.push(text.to_owned());
        }
        async fn flush(&mut self) {
            self.lines.push("<flush>".to_owned());
        }
    }

    struct Echo;

    static ECHO_HELP: [Help; 1] = [Help {
        usage: "echo <text>",
        summary: "write the text back",
        secret: false,
    }];

    impl Commands for Echo {
        fn help(&self) -> impl Iterator<Item = &'static Help> {
            ECHO_HELP.iter()
        }
        async fn run<R: Reply>(
            &mut self,
            name: &str,
            mut args: Args<'_>,
            reply: &mut R,
            _: Cancel<'_>,
        ) -> CommandResult {
            match name {
                "echo" => match args.rest() {
                    Some(text) => {
                        reply.line(text).await;
                        CommandResult::Ok
                    }
                    None => CommandResult::Err("echo needs text"),
                },
                _ => CommandResult::Unknown,
            }
        }
    }

    struct Count(u32);

    static COUNT_HELP: [Help; 1] = [Help {
        usage: "count",
        summary: "write how often it ran",
        secret: false,
    }];

    impl Commands for Count {
        fn help(&self) -> impl Iterator<Item = &'static Help> {
            COUNT_HELP.iter()
        }
        async fn run<R: Reply>(
            &mut self,
            name: &str,
            _: Args<'_>,
            reply: &mut R,
            _: Cancel<'_>,
        ) -> CommandResult {
            if name != "count" {
                return CommandResult::Unknown;
            }
            self.0 += 1;
            reply.field("count", self.0).await;
            CommandResult::Ok
        }
    }

    fn never() -> ! {
        panic!("reset")
    }

    fn run<C: Commands>(commands: &mut C, line: Result<&str, LineError>) -> Vec<String> {
        let mut reply = Recorder::default();
        block_on(execute(
            commands,
            line,
            &mut reply,
            never,
            CancelSignal::new().cancel(),
        ));
        reply.lines
    }

    #[test]
    fn a_command_writes_its_body_then_ok() {
        assert_eq!(
            run(&mut Echo, Ok("echo hello  world")),
            ["hello  world", "ok"]
        );
    }

    #[test]
    fn a_failed_command_writes_err_and_its_reason() {
        assert_eq!(run(&mut Echo, Ok("echo")), ["err echo needs text"]);
    }

    #[test]
    fn an_unknown_command_is_named() {
        assert_eq!(
            run(&mut Echo, Ok("  frob 1 2")),
            ["err unknown command frob"]
        );
    }

    #[test]
    fn an_empty_line_writes_nothing() {
        assert_eq!(run(&mut Echo, Ok("")), Vec::<String>::new());
        assert_eq!(run(&mut Echo, Ok("   ")), Vec::<String>::new());
    }

    #[test]
    fn a_refused_line_writes_its_err() {
        assert_eq!(
            run(&mut Echo, Err(LineError::TooLong)),
            ["err line too long"]
        );
        assert_eq!(run(&mut Echo, Err(LineError::NotUtf8)), ["err not utf-8"]);
    }

    #[test]
    fn help_lists_the_built_ins_then_every_set_in_order() {
        let mut both = (Echo, Count(0));
        assert_eq!(
            run(&mut both, Ok("help")),
            [
                "help: list the commands",
                "reset: reset the chip",
                "echo <text>: write the text back",
                "count: write how often it ran",
                "ok",
            ]
        );
    }

    #[test]
    fn the_first_set_that_knows_a_name_runs_it_and_state_is_kept() {
        let mut both = (Echo, Count(0));
        assert_eq!(run(&mut both, Ok("count")), ["count: 1", "ok"]);
        assert_eq!(run(&mut both, Ok("count")), ["count: 2", "ok"]);
        assert_eq!(run(&mut both, Ok("echo x")), ["x", "ok"]);
        assert_eq!(run(&mut both, Ok("nope")), ["err unknown command nope"]);
    }

    #[test]
    fn the_empty_set_knows_nothing() {
        assert_eq!(
            run(&mut (), Ok("help")),
            ["help: list the commands", "reset: reset the chip", "ok"]
        );
        assert_eq!(run(&mut (), Ok("echo")), ["err unknown command echo"]);
    }

    #[test]
    fn reset_writes_ok_and_flushes_before_it_resets() {
        let mut reply = Recorder::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            block_on(execute(
                &mut Echo,
                Ok("reset"),
                &mut reply,
                never,
                CancelSignal::new().cancel(),
            ))
        }));
        assert!(result.is_err());
        assert_eq!(reply.lines, ["ok", "<flush>"]);
    }

    #[test]
    fn a_byte_is_masked_once_the_secret_argument_of_any_set_has_begun() {
        struct Join;
        static JOIN_HELP: [Help; 1] = [Help {
            usage: "join <ssid> <password>",
            summary: "",
            secret: true,
        }];
        impl Commands for Join {
            fn help(&self) -> impl Iterator<Item = &'static Help> {
                JOIN_HELP.iter()
            }
            async fn run<R: Reply>(
                &mut self,
                _: &str,
                _: Args<'_>,
                _: &mut R,
                _: Cancel<'_>,
            ) -> CommandResult {
                CommandResult::Unknown
            }
        }
        let both = (Echo, Join);
        let mut input = Input::<32>::new();
        let mut echoed = String::new();
        for byte in b"join home  p w\r" {
            if input.push(*byte) == Action::Echo(*byte) {
                echoed.push(if masked(&both, &input) {
                    '*'
                } else {
                    *byte as char
                });
            }
        }
        assert_eq!(echoed, "join home  ***");
        assert_eq!(input.take(), Ok("join home  p w"));
        for byte in b"echo home p" {
            input.push(*byte);
            assert!(!masked(&both, &input));
        }
    }
}
