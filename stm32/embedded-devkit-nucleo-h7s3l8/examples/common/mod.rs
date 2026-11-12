//! What the console examples share. It sits in a folder, so Cargo does not take it for an example of its own.
//! Each board crate holds its own copy, so an application can start from one board crate alone.

use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};

/// The name the application gives `info`.
pub const FIRMWARE: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

/// The `echo` command, which writes its text back.
pub struct Echo;

static HELP: [Help; 1] = [Help {
    usage: "echo <text>",
    summary: "write the text back",
    secret: false,
}];

impl Commands for Echo {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        HELP.iter()
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        mut args: Args<'_>,
        reply: &mut R,
        _: Cancel<'_>,
    ) -> CommandResult {
        match name {
            "echo" => {
                reply.line(args.rest().unwrap_or("")).await;
                CommandResult::Ok
            }
            _ => CommandResult::Unknown,
        }
    }
}
