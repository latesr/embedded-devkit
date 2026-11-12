//! The `mcu-run` command: `run` flashes and runs a firmware or an embedded-test binary, `console` talks to the
//! board's console, and `list` shows the attached boards.

use clap::{Args, Parser, Subcommand};
use mcu_run::{Board, Client, CommandResult, Error, FlashMode, Image, Runner, TestResult};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// The exit code for a failure of `mcu-run` itself, apart from the firmware's own codes, 124 at a deadline
/// and 1 for a failed test or an `err` result line.
const FAILED: u8 = 2;

#[derive(Parser)]
#[command(
    name = "mcu",
    version,
    about = "Flashes a board, runs the firmware and drives its console"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Flash an ELF, run it and follow its output. An embedded-test binary runs test by test instead.
    Run(RunArgs),
    /// Talk to the board's console over its serial port. Interactive unless a line is given to send.
    Console(ConsoleArgs),
    /// List the attached boards with their selectors and serial ports.
    List(ListArgs),
}

#[derive(Args)]
struct RunArgs {
    /// The board, as `probe://<part of its probe-rs selector>`, ignoring case. Default: the one board attached, or
    /// the one board with the ELF's chip.
    #[arg(short, long, env = "MCU_DEVICE")]
    device: Option<String>,
    /// The chip's name in probe-rs, such as esp32c5 or STM32H7S3L8. Default: the name in the ELF's
    /// `.probe-rs.chip` section.
    #[arg(long)]
    chip: Option<String>,
    /// A chip description in probe-rs's YAML, loaded in place of the built-in one for its family.
    #[arg(long)]
    chip_description: Option<PathBuf>,
    /// The flash algorithm that writes its range of flash, by its name in the chip description. The other
    /// algorithms for that range are removed. Default: the name in the ELF's `.mcu-run.flash-algorithm`
    /// section, or else probe-rs chooses.
    #[arg(long)]
    prefer_flash_algorithm: Option<String>,
    /// The ESP-IDF partition table CSV. Default: probe-rs's own table.
    #[arg(long)]
    partition_table: Option<PathBuf>,
    /// The partition the app goes into, by its name in the table. Default: the first app partition.
    #[arg(long)]
    app_partition: Option<String>,
    /// The bootloader to flash with the app. Default: probe-rs's built-in one for the chip.
    #[arg(long)]
    bootloader: Option<PathBuf>,
    /// Skip the flash pages that already hold the image's bytes.
    #[arg(long)]
    preverify: bool,
    /// Read the flash back after the write.
    #[arg(long)]
    verify: bool,
    /// The SPI flash mode an ESP32 boots in: qio, qout, dio or dout. Default: the mode in the ELF's
    /// `.mcu-run.flash-mode` section, or else qio. A board with a flash that cannot run quad does not boot in qio.
    #[arg(long)]
    flash_mode: Option<FlashMode>,
    /// Stop with exit code 0 at the first output line that contains this text. A defmt line ends with its
    /// source location, which counts too. Ignored for an embedded-test binary.
    #[arg(long)]
    until: Option<String>,
    /// Stop with exit code 124 after this many seconds. Ignored for an embedded-test binary, whose tests have
    /// their own timeouts.
    #[arg(long)]
    timeout: Option<u64>,
    /// Write the board's output to this file as well.
    #[arg(long)]
    log: Option<PathBuf>,
    /// Start the firmware and open its console in place of its output, as `mcu console` does.
    #[arg(long, conflicts_with_all = ["until", "timeout", "log"])]
    console: bool,
    /// The ELF to flash.
    elf: PathBuf,
}

#[derive(Args)]
struct ListArgs {
    /// Open each probe and show the chip at it, from its JTAG IDCODE, or `?`. The chips keep running.
    #[arg(short, long)]
    chip: bool,
}

#[derive(Args)]
struct ConsoleArgs {
    /// The board, as `probe://<part of its probe-rs selector>`, ignoring case. Optional with one board attached.
    #[arg(short, long, env = "MCU_DEVICE")]
    device: Option<String>,
    /// Send this one line, print the body and the result line, and exit 0 on ok, 1 on err and 124 on a timeout.
    #[arg(long)]
    send: Option<String>,
    /// How many seconds to wait for the result line.
    #[arg(long, default_value_t = 5)]
    timeout: u64,
    /// Send Ctrl-C this many seconds after the line, to stop a command that runs until stopped, and a second at
    /// twice that to drop one that does not stop. The timeout also counts from the line, so it must be longer.
    #[arg(long, requires = "send")]
    interrupt_after: Option<u64>,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => run(args),
        Command::Console(args) => console(args),
        Command::List(args) => list(&args),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error @ Error::NoChip { .. }) => {
            eprintln!("mcu: {error}. Pass --chip to name it.");
            ExitCode::from(FAILED)
        }
        Err(error @ Error::ManyBoards { .. }) => {
            eprintln!("mcu: {error}\n`mcu list --chip` shows the chip at each probe.");
            ExitCode::from(FAILED)
        }
        Err(error) => {
            eprintln!("mcu: {error}");
            ExitCode::from(if matches!(error, Error::Timeout { .. }) {
                124
            } else {
                FAILED
            })
        }
    }
}

fn board(device: Option<&str>) -> Result<Board, Error> {
    mcu_run::select(&mcu_run::attached(), device).cloned()
}

/// The image that `args` name: the ELF's own chip and flash algorithm, each replaced by its option when given.
fn image(args: &RunArgs) -> Result<Image, Error> {
    let named = match Image::from_elf(args.elf.clone()) {
        Ok(image) => image,
        Err(Error::NoChip { .. }) if args.chip.is_some() => Image {
            elf: args.elf.clone(),
            ..Image::default()
        },
        Err(error) => return Err(error),
    };
    Ok(Image {
        chip: args.chip.clone().unwrap_or(named.chip),
        chip_description: args.chip_description.clone(),
        preferred_flash_algorithm: args
            .prefer_flash_algorithm
            .clone()
            .or(named.preferred_flash_algorithm),
        partition_table: args.partition_table.clone(),
        app_partition: args.app_partition.clone(),
        bootloader: args.bootloader.clone(),
        preverify: args.preverify,
        verify: args.verify,
        flash_mode: args.flash_mode.or(named.flash_mode),
        elf: named.elf,
    })
}

fn run(args: RunArgs) -> Result<u8, Error> {
    let runner = Runner::new(image(&args)?)?;
    if args.console {
        if runner.is_test_binary()? {
            eprintln!(
                "mcu: {} is an embedded-test binary, which runs without a console",
                args.elf.display()
            );
            return Ok(FAILED);
        }
        let board = runner.board(args.device.as_deref())?;
        // A person can still talk to firmware of another protocol version, so the mismatch is a warning here.
        match runner.console(&board) {
            Ok(client) => drop(client),
            Err(error @ Error::Protocol { .. }) => eprintln!("mcu: warning: {error}"),
            Err(error) => return Err(error),
        }
        return interactive(&board, mcu_run::port(&board)?);
    }
    let board = runner.board(args.device.as_deref())?;
    if runner.is_test_binary()? {
        let results = runner.test(&board, args.log.as_deref())?;
        let failed = results
            .iter()
            .any(|result| matches!(result, TestResult::Failed { .. }));
        return Ok(u8::from(failed));
    }
    let stop = runner.run(
        &board,
        args.until.as_deref(),
        args.timeout.map(Duration::from_secs),
        args.log.as_deref(),
    )?;
    Ok(stop.code())
}

fn console(args: ConsoleArgs) -> Result<u8, Error> {
    let board = board(args.device.as_deref())?;
    let path = mcu_run::port(&board)?;
    match args.send {
        Some(line) => {
            let mut client = Client::new(mcu_run::open_port(&path)?);
            let timeout = Duration::from_secs(args.timeout);
            let reply = match args.interrupt_after {
                Some(after) => {
                    client.send_interrupted(&line, Duration::from_secs(after), timeout)?
                }
                None => client.send(&line, timeout)?,
            };
            for body in &reply.body {
                println!("{body}");
            }
            Ok(match reply.result {
                CommandResult::Ok => {
                    println!("ok");
                    0
                }
                CommandResult::Err(reason) => {
                    println!("err {reason}");
                    1
                }
            })
        }
        None => interactive(&board, path),
    }
}

/// How long the console waits for the board's port to return after it went, as a reset button's reset takes it.
const PORT_RETURN: Duration = Duration::from_secs(10);

/// Ctrl-C, which the console answers with a fresh prompt and no command (K-14).
const FRESH_PROMPT: &[u8] = b"\x03";

/// Copies the console to the terminal and each keystroke to the console, in raw mode, until Ctrl-] or Ctrl-D.
/// It sends Ctrl-C on connecting, so the console writes a prompt. When the port goes, as it does when the board's
/// reset button resets the chip and its USB, it waits for the port to return and connects again (M-5, M-17).
fn interactive(board: &Board, mut path: String) -> Result<u8, Error> {
    use crossterm::event::Event;
    eprintln!(
        "mcu: console of {} on {path}. Ctrl-] or Ctrl-D exits.",
        board.selector
    );
    let mut transport = connect(&path)?;
    crossterm::terminal::enable_raw_mode().map_err(|error| Error::Transport(error.to_string()))?;
    let result = (|| {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 256];
        loop {
            let gone = match transport.read(&mut buf) {
                Ok(0) => true,
                Ok(n) => {
                    stdout
                        .write_all(buf.get(..n).unwrap_or(&[]))
                        .and_then(|()| stdout.flush())
                        .map_err(|error| Error::Output(error.to_string()))?;
                    false
                }
                Err(error) => error.kind() != std::io::ErrorKind::TimedOut,
            };
            if gone {
                eprint!("\r\n(the board reset, waiting for it to return)\r\n");
                path = mcu_run::wait_for_port(board, PORT_RETURN)?;
                transport = connect(&path)?;
                continue;
            }
            while crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
                let Ok(Event::Key(event)) = crossterm::event::read() else {
                    continue;
                };
                let bytes = match key(event) {
                    Key::Quit => return Ok(0),
                    Key::Send(bytes) => bytes,
                    Key::Ignore => continue,
                };
                // A write to a port that went fails here, and the next read finds it gone.
                let _ = transport.write_all(&bytes).and_then(|()| transport.flush());
            }
        }
    })();
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result
}

/// Opens the console's port and asks for a fresh prompt.
fn connect(path: &str) -> Result<impl Read + Write + use<>, Error> {
    let mut transport = mcu_run::open_port(path)?;
    transport
        .write_all(FRESH_PROMPT)
        .and_then(|()| transport.flush())
        .map_err(|error| Error::Transport(error.to_string()))?;
    Ok(transport)
}

fn list(args: &ListArgs) -> Result<u8, Error> {
    let boards = if args.chip {
        mcu_run::attached_with_chips()
    } else {
        mcu_run::attached()
    };
    for board in boards {
        let port = match mcu_run::port(&board) {
            Ok(port) => match mcu_run::port_holders(&port).as_slice() {
                [] => port,
                holders => format!("{port} (held by {})", holders.join(", ")),
            },
            Err(_) => "(no serial port)".to_owned(),
        };
        if args.chip {
            let chip = board.chip.as_deref().unwrap_or("?");
            println!("{}  {}  {chip}  {port}", board.selector, board.name);
        } else {
            println!("{}  {}  {port}", board.selector, board.name);
        }
    }
    Ok(0)
}

/// What one key press does in the interactive console.
#[derive(Debug, PartialEq, Eq)]
enum Key {
    Quit,
    Send(Vec<u8>),
    Ignore,
}

/// The console's answer to `event`. Ctrl-] and Ctrl-D quit. crossterm reports the bytes 0x1C to 0x1F as Ctrl
/// with the digits 4 to 7, so a terminal's Ctrl-], the byte 0x1D, arrives as Ctrl-5.
fn key(event: crossterm::event::KeyEvent) -> Key {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    if event.kind == KeyEventKind::Release {
        return Key::Ignore;
    }
    let control = event.modifiers.contains(KeyModifiers::CONTROL);
    match event.code {
        KeyCode::Char(']' | '5' | 'd') if control => Key::Quit,
        // Ctrl-A to Ctrl-Z are the bytes 1 to 26.
        KeyCode::Char(c @ 'a'..='z') if control => {
            Key::Send(vec![(c as u8).wrapping_sub(b'a').wrapping_add(1)])
        }
        KeyCode::Char(c) => Key::Send(c.to_string().into_bytes()),
        KeyCode::Enter => Key::Send(b"\r".to_vec()),
        KeyCode::Backspace => Key::Send(b"\x7f".to_vec()),
        KeyCode::Tab => Key::Send(b"\t".to_vec()),
        _ => Key::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_close_bracket_quits_however_the_terminal_reports_it() {
        let control = KeyModifiers::CONTROL;
        assert_eq!(key(press(KeyCode::Char(']'), control)), Key::Quit);
        assert_eq!(key(press(KeyCode::Char('5'), control)), Key::Quit);
        assert_eq!(key(press(KeyCode::Char('d'), control)), Key::Quit);
    }

    #[test]
    fn keys_become_the_bytes_the_console_reads() {
        let none = KeyModifiers::NONE;
        assert_eq!(
            key(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Key::Send(vec![3])
        );
        assert_eq!(
            key(press(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            Key::Send(vec![1])
        );
        assert_eq!(
            key(press(KeyCode::Char('é'), none)),
            Key::Send("é".as_bytes().to_vec())
        );
        assert_eq!(
            key(press(KeyCode::Char('5'), none)),
            Key::Send(b"5".to_vec())
        );
        assert_eq!(key(press(KeyCode::Enter, none)), Key::Send(b"\r".to_vec()));
        assert_eq!(
            key(press(KeyCode::Backspace, none)),
            Key::Send(b"\x7f".to_vec())
        );
        assert_eq!(key(press(KeyCode::Tab, none)), Key::Send(b"\t".to_vec()));
        assert_eq!(key(press(KeyCode::F(1), none)), Key::Ignore);
        let mut release = press(KeyCode::Char('a'), none);
        release.kind = KeyEventKind::Release;
        assert_eq!(key(release), Key::Ignore);
    }

    #[test]
    fn run_takes_its_options_before_or_after_the_elf() {
        let cli = Cli::try_parse_from([
            "mcu",
            "run",
            "--chip",
            "esp32c5",
            "app.elf",
            "--until",
            "done",
            "--timeout",
            "5",
            "--preverify",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("not run");
        };
        assert_eq!(args.chip.as_deref(), Some("esp32c5"));
        assert_eq!(args.elf, PathBuf::from("app.elf"));
        assert_eq!(args.until.as_deref(), Some("done"));
        assert_eq!(args.timeout, Some(5));
        assert!(args.preverify);
        assert!(!args.verify);
        assert_eq!(args.device, None);
    }

    #[test]
    fn list_opens_the_probes_only_when_asked_for_the_chip() {
        for (args, chip) in [
            (&["mcu", "list"][..], false),
            (&["mcu", "list", "-c"][..], true),
            (&["mcu", "list", "--chip"][..], true),
        ] {
            let Command::List(list) = Cli::try_parse_from(args).unwrap().command else {
                panic!("not list");
            };
            assert_eq!(list.chip, chip, "{args:?}");
        }
    }

    #[test]
    fn run_opens_the_console_only_without_a_stop_or_a_log() {
        let cli = Cli::try_parse_from(["mcu", "run", "--console", "app.elf"]).unwrap();
        let Command::Run(args) = cli.command else {
            panic!("not run");
        };
        assert!(args.console);
        let plain = Cli::try_parse_from(["mcu", "run", "app.elf"]).unwrap();
        let Command::Run(args) = plain.command else {
            panic!("not run");
        };
        assert!(!args.console);
        for (option, value) in [
            ("--until", "done"),
            ("--timeout", "5"),
            ("--log", "out.log"),
        ] {
            assert!(
                Cli::try_parse_from(["mcu", "run", "--console", option, value, "app.elf"]).is_err(),
                "{option}"
            );
        }
    }

    #[test]
    fn the_flash_mode_option_replaces_the_elf_s_mode() {
        use object::write::Object;
        use object::{Architecture, BinaryFormat, Endianness, SectionKind};
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        for (name, data) in [
            (".probe-rs.chip", &b"esp32c5"[..]),
            (".mcu-run.flash-mode", b"dio"),
        ] {
            let section =
                object.add_section(vec![], name.as_bytes().to_vec(), SectionKind::Metadata);
            object.append_section_data(section, data, 1);
        }
        let elf =
            std::env::temp_dir().join(format!("mcu-run-main-{}-mode.elf", std::process::id()));
        std::fs::write(&elf, object.write().unwrap()).unwrap();
        let mode = |extra: &[&str]| {
            let mut words = vec!["mcu", "run"];
            words.extend_from_slice(extra);
            words.push(elf.to_str().unwrap());
            let Command::Run(args) = Cli::try_parse_from(words).unwrap().command else {
                panic!("not run");
            };
            image(&args).unwrap().flash_mode
        };
        assert_eq!(mode(&[]), Some(FlashMode::Dio));
        assert_eq!(mode(&["--flash-mode", "qout"]), Some(FlashMode::Qout));
        assert!(Cli::try_parse_from(["mcu", "run", "--flash-mode", "fast", "x.elf"]).is_err());
        std::fs::remove_file(&elf).unwrap();
    }

    #[test]
    fn run_needs_the_elf_and_not_the_chip() {
        let cli = Cli::try_parse_from(["mcu", "run", "app.elf"]).unwrap();
        let Command::Run(args) = cli.command else {
            panic!("not run");
        };
        assert_eq!(args.chip, None);
        assert!(Cli::try_parse_from(["mcu", "run", "--chip", "esp32c5"]).is_err());
    }

    #[test]
    fn console_takes_a_line_to_send_and_a_timeout() {
        let cli =
            Cli::try_parse_from(["mcu", "console", "-d", "probe://303a", "--send", "echo hi"])
                .unwrap();
        let Command::Console(args) = cli.command else {
            panic!("not console");
        };
        assert_eq!(args.device.as_deref(), Some("probe://303a"));
        assert_eq!(args.send.as_deref(), Some("echo hi"));
        assert_eq!(args.timeout, 5);
        assert_eq!(args.interrupt_after, None);
    }

    #[test]
    fn console_sends_ctrl_c_after_a_wait_only_with_a_line_to_send() {
        let cli = Cli::try_parse_from([
            "mcu",
            "console",
            "--send",
            "net ping 10.0.0.1 1000",
            "--interrupt-after",
            "3",
        ])
        .unwrap();
        let Command::Console(args) = cli.command else {
            panic!("not console");
        };
        assert_eq!(args.interrupt_after, Some(3));
        assert!(Cli::try_parse_from(["mcu", "console", "--interrupt-after", "3"]).is_err());
    }
}
