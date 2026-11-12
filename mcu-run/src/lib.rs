//! Flashes a board, runs the firmware and drives its console from the host.
//!
//! The library holds no probe-rs type in its API. A host test uses it without depending on probe-rs.

mod board;
mod chip;
mod client;
mod detect;
mod monitor;
mod probe_thread;
mod runner;
mod serial;
mod test;

pub use board::{Board, attached, attached_with_chips, select};
pub use client::{Banner, Client, CommandResult, Reply};
pub use monitor::Stop;
pub use runner::{FlashMode, Image, Runner, Transport};
pub use serial::{open_port, port, port_holders, wait_for_port};
pub use test::TestResult;

/// The version of the console protocol that this library speaks, which a console's banner names as
/// `protocol <v>` (K-9). [`Runner::console`] refuses a console with another version.
pub const PROTOCOL: u32 = 1;

/// Registers probe-rs's Espressif plugin once. It brings the ESP USB JTAG probe driver, the ESP32 chip
/// descriptions and the ESP-IDF image loader, and registering it twice would list each probe twice.
pub(crate) fn register_espressif() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(probe_rs_espressif::register_plugin);
}

/// Every failure the library reports.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The serial ports could not be listed. The text is the operating system's error.
    Ports(String),
    /// No serial port has the board's USB serial number. The list holds every USB serial port, as
    /// `<path>  <serial number>`.
    NoPort {
        /// The serial number that was looked for.
        serial: String,
        /// The USB serial ports that were found.
        ports: Vec<String>,
    },
    /// More than one serial port has the board's USB serial number.
    ManyPorts {
        /// The serial number that was looked for.
        serial: String,
        /// The paths of the ports that have it.
        paths: Vec<String>,
    },
    /// Another process holds the board's console port, so a flash or a reset would disturb it.
    PortInUse {
        /// The path of the port.
        port: String,
        /// Each process that holds it, as `<command> (pid <n>)`.
        holders: Vec<String>,
    },
    /// The board's serial port did not return within the wait.
    PortGone {
        /// The serial number that was looked for.
        serial: String,
        /// How long the wait was.
        waited: std::time::Duration,
    },
    /// The chip description file could not be read, or probe-rs refused it.
    ChipDescription {
        /// The file.
        path: std::path::PathBuf,
        /// What went wrong, from the operating system or probe-rs.
        error: String,
    },
    /// probe-rs has no chip of that name.
    UnknownChip {
        /// The name that was asked for.
        chip: String,
        /// probe-rs's error, which lists chips with a similar name.
        error: String,
    },
    /// The chip description does not have a flash algorithm of the preferred name.
    UnknownFlashAlgorithm {
        /// The chip, as its description spells it.
        chip: String,
        /// The name that was asked for.
        algorithm: String,
    },
    /// The log file could not be created or written.
    Log {
        /// The file.
        path: std::path::PathBuf,
        /// The operating system's error.
        error: String,
    },
    /// The board's output could not be written to the screen.
    Output(String),
    /// The ELF file could not be read, or is not an ELF.
    Elf {
        /// The file.
        path: std::path::PathBuf,
        /// The operating system's or the parser's error.
        error: String,
    },
    /// The ELF names no chip: its `.probe-rs.chip` section is missing or empty.
    NoChip {
        /// The file.
        elf: std::path::PathBuf,
    },
    /// The ELF's defmt table or its locations could not be read.
    Defmt(String),
    /// The probe could not be opened or attached to the chip.
    Probe(String),
    /// The flash write failed.
    Flash(String),
    /// The core could not be reset, run or read.
    Run(String),
    /// The embedded-test list in the ELF cannot be read, or the ELF holds none.
    TestList(String),
    /// The console transport failed, or the console closed it.
    Transport(String),
    /// The console did not answer within the timeout.
    Timeout {
        /// What the client waited for: the result line or the banner.
        waiting_for: String,
        /// The bytes of a line that had not ended when the timeout passed.
        partial: String,
    },
    /// No attached board matches the device value. The list holds every board that was found.
    NoBoard {
        /// The device value that matched nothing, such as `probe://0483:3754`, or `None` when none was given.
        selector: Option<String>,
        /// The boards that were found.
        boards: Vec<Board>,
    },
    /// More than one attached board matches the device value, or none was given and more than one board is
    /// attached. The list holds the boards that match.
    ManyBoards {
        /// The device value that matched them, or `None` when none was given.
        selector: Option<String>,
        /// The boards that match.
        boards: Vec<Board>,
    },
    /// The device value names a kind of device that this library does not support yet, such as `tcp://`.
    UnsupportedDevice {
        /// The device value.
        device: String,
    },
    /// The device value is a bare name, and no devices file names that device.
    NoSuchDevice {
        /// The device value.
        device: String,
    },
    /// The console's banner names another protocol version than [`PROTOCOL`], or none.
    Protocol {
        /// The version in the banner, or `None` for a banner without one.
        firmware: Option<u32>,
        /// The version this library speaks, [`PROTOCOL`].
        host: u32,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoBoard { selector, boards } => {
                match selector {
                    Some(selector) => write!(f, "no attached board matches {selector}.")?,
                    None => write!(f, "no board is attached.")?,
                }
                list(f, " Boards found:", boards)
            }
            Error::ManyBoards { selector, boards } => {
                match selector {
                    Some(selector) => write!(f, "{selector} matches {} boards.", boards.len())?,
                    None => write!(f, "{} boards are attached.", boards.len())?,
                }
                list(f, " Name one with probe://<part of its selector>:", boards)
            }
            Error::UnsupportedDevice { device } => {
                write!(f, "{device}: this kind of device is not supported yet")
            }
            Error::NoSuchDevice { device } => write!(
                f,
                "no device named {device}: there is no devices file yet. A probe is probe://<part of its selector>"
            ),
            Error::Ports(error) => write!(f, "could not list the serial ports: {error}"),
            Error::NoPort { serial, ports } => {
                write!(f, "no serial port has the serial number {serial}.")?;
                if ports.is_empty() {
                    return Ok(());
                }
                write!(f, " USB serial ports found:")?;
                for port in ports {
                    write!(f, "\n  {port}")?;
                }
                Ok(())
            }
            Error::ManyPorts { serial, paths } => write!(
                f,
                "{} serial ports have the serial number {serial}: {}",
                paths.len(),
                paths.join(", ")
            ),
            Error::PortInUse { port, holders } => write!(
                f,
                "{port} is held by {}. Close it, or name another board with -d probe://<part of its selector>",
                holders.join(", ")
            ),
            Error::PortGone { serial, waited } => write!(
                f,
                "the serial port with the serial number {serial} did not return within {waited:?}"
            ),
            Error::ChipDescription { path, error } => {
                write!(f, "chip description {}: {error}", path.display())
            }
            Error::UnknownChip { chip, error } => write!(f, "unknown chip {chip}: {error}"),
            Error::UnknownFlashAlgorithm { chip, algorithm } => {
                write!(f, "{chip} has no flash algorithm named {algorithm}")
            }
            Error::Log { path, error } => write!(f, "log {}: {error}", path.display()),
            Error::Output(error) => write!(f, "could not write the output: {error}"),
            Error::Elf { path, error } => write!(f, "{}: {error}", path.display()),
            Error::NoChip { elf } => write!(
                f,
                "{} names no chip in a .probe-rs.chip section",
                elf.display()
            ),
            Error::Defmt(error) => write!(f, "defmt: {error}"),
            Error::Probe(error) => write!(f, "probe: {error}"),
            Error::Flash(error) => write!(f, "flashing: {error}"),
            Error::Run(error) => write!(f, "running: {error}"),
            Error::TestList(error) => write!(f, "embedded-test: {error}"),
            Error::Transport(error) => write!(f, "console transport: {error}"),
            Error::Timeout {
                waiting_for,
                partial,
            } => {
                write!(f, "{waiting_for} did not arrive within the timeout.")?;
                if !partial.is_empty() {
                    write!(f, " Unfinished line: {partial}")?;
                }
                Ok(())
            }
            Error::Protocol {
                firmware: Some(firmware),
                host,
            } => write!(
                f,
                "the firmware speaks console protocol {firmware}, and this mcu-run speaks {host}. Build the \
                 firmware's mcu-console and the host's mcu-run from one release"
            ),
            Error::Protocol {
                firmware: None,
                host,
            } => write!(
                f,
                "the firmware's banner names no console protocol, so its mcu-console is older than protocol \
                 {host}, which this mcu-run speaks"
            ),
        }
    }
}

impl std::error::Error for Error {}

fn list(f: &mut std::fmt::Formatter<'_>, heading: &str, boards: &[Board]) -> std::fmt::Result {
    if boards.is_empty() {
        return Ok(());
    }
    write!(f, "{heading}")?;
    for board in boards {
        write!(f, "\n  {}  {}", board.selector, board.name)?;
        if let Some(chip) = &board.chip {
            write!(f, "  {chip}")?;
        }
    }
    Ok(())
}
