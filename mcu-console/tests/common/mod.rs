//! What the blocking and the async driver tests share: the command set, the configuration and the delay. The
//! fakes for the transport stay in each test file, because one is blocking and the other is async.

use mcu_console::{Args, Cancel, CommandResult, Commands, Config, Help, Reply};
use std::cell::RefCell;
use std::rc::Rc;

/// Counts the milliseconds the console waited. Each wait completes at once.
#[derive(Default)]
pub struct Delay(pub Rc<RefCell<u32>>);

impl embedded_hal::delay::DelayNs for Delay {
    fn delay_ns(&mut self, ns: u32) {
        *self.0.borrow_mut() += ns / 1_000_000;
    }
}

impl embedded_hal_async::delay::DelayNs for Delay {
    async fn delay_ns(&mut self, ns: u32) {
        *self.0.borrow_mut() += ns / 1_000_000;
    }
}

/// `echo`, `join` with a secret password, and three commands that are not in the help: `long` writes a 300-byte
/// body line, `wait` writes `stopped` once asked to stop, and `hang` writes `busy` and never returns.
pub struct Wifi;

pub static WIFI_HELP: [Help; 2] = [
    Help {
        usage: "echo <text>",
        summary: "write the text back",
        secret: false,
    },
    Help {
        usage: "join <ssid> <password>",
        summary: "join a network",
        secret: true,
    },
];

impl Commands for Wifi {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        WIFI_HELP.iter()
    }
    async fn run<R: Reply>(
        &mut self,
        name: &str,
        mut args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        match name {
            "echo" => {
                reply.line(args.rest().unwrap_or("")).await;
                CommandResult::Ok
            }
            "join" => {
                let ssid = args.next().unwrap_or("");
                reply.field("ssid", ssid).await;
                reply.field("password", args.rest().unwrap_or("")).await;
                CommandResult::Ok
            }
            "long" => {
                reply.line(&"x".repeat(300)).await;
                CommandResult::Ok
            }
            "wait" => {
                cancel.requested().await;
                reply.line("stopped").await;
                CommandResult::Ok
            }
            "hang" => {
                reply.line("busy").await;
                std::future::pending().await
            }
            _ => CommandResult::Unknown,
        }
    }
}

pub fn never() -> ! {
    panic!("reset")
}

pub const CONFIG: Config = Config {
    name: "mcu",
    boot: 3,
    write_timeout_ms: 500,
    reset: never,
    image: None,
};

/// The output for `long`: the echo, 256 of the 300 bytes, the result line and the prompt (K-21).
pub fn long_line_output() -> String {
    format!(
        "\r\nmcu boot 3 protocol 1\r\nmcu> long\r\n{}\r\nok\r\nmcu> ",
        "x".repeat(256)
    )
}
