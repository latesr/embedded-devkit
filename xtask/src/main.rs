//! Repository tasks. `cargo xtask <command>` runs one.

mod check;
mod hil;
mod iperf;
mod shared;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about = "Repository tasks", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the quick checks that the change reaches, with every feature on and then off.
    Check {
        /// Also run the slow checks, over what the branch changed since its upstream or origin/HEAD. The pre-push
        /// hook uses it.
        #[arg(long, conflicts_with = "full")]
        push: bool,
        /// Run every check on every crate, also over each feature alone and each pair, as the nightly CI job does.
        #[arg(long)]
        full: bool,
    },
    /// Run the hardware tests on one board: its board crate's on-target tests, then the scenario tests. Needs the
    /// board.
    Hil {
        /// The board crate's folder, such as esp/embedded-devkit-esp32-c5-devkitc-1. Needed when more than one board
        /// crate is listed.
        #[arg(long)]
        board: Option<String>,
        /// The board, as `probe://<part of its probe-rs selector>`, ignoring case. Needed when two attached boards
        /// have the board crate's chip.
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Run one iperf test as the server on this host, the scenario tests' host end, then exit.
    IperfServer {
        /// A UDP test, not TCP.
        #[arg(short = 'u')]
        udp: bool,
        /// The port to listen on, or 0 for a free one.
        #[arg(short = 'p', default_value_t = 5001)]
        port: u16,
    },
    #[command(flatten)]
    Shared(shared::Command),
}

fn main() {
    let cli = Cli::parse();
    let root = repo_root();
    let result = match cli.command {
        Command::Check { push, full } => check::run(
            &root,
            match (push, full) {
                (_, true) => check::Scope::Full,
                (true, false) => check::Scope::Push,
                (false, false) => check::Scope::Commit,
            },
        ),
        Command::Hil { board, device } => hil::run(&root, board.as_deref(), device.as_deref()),
        Command::IperfServer { udp, port } => {
            let transport = if udp {
                embedded_iperf2::Transport::Udp
            } else {
                embedded_iperf2::Transport::Tcp
            };
            iperf::run(transport, port)
        }
        Command::Shared(command) => shared::run(&root, command),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

/// The workspace root, which is the parent of the `xtask` crate.
fn repo_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.to_path_buf())
}
