//! Finding a board's console serial port from its USB serial number.

use crate::{Board, Error};
use std::time::{Duration, Instant};

/// How often a gone port is looked for.
const POLL: Duration = Duration::from_millis(250);

/// One serial port on a USB device.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortInfo {
    path: String,
    serial: Option<String>,
}

impl Board {
    /// The serial number in the selector: the part after `VID:PID:`. Empty when the selector has none.
    pub fn serial(&self) -> &str {
        self.selector.splitn(3, ':').nth(2).unwrap_or("")
    }
}

/// The path of the board's console serial port, from the USB device with the board's serial number.
///
/// Fails with [`Error::NoPort`] when no port has the serial number, and with [`Error::ManyPorts`] when more
/// than one does.
pub fn port(board: &Board) -> Result<String, Error> {
    choose(&ports()?, board.serial())
}

/// The path of the board's console serial port, waiting up to `timeout` for the port to appear. A reset that
/// drops the USB link takes the port away for a moment, and this waits for it to return.
///
/// Fails with [`Error::PortGone`] when the port has not returned by the deadline.
pub fn wait_for_port(board: &Board, timeout: Duration) -> Result<String, Error> {
    let start = Instant::now();
    wait(
        board.serial(),
        timeout,
        ports,
        || start.elapsed(),
        || std::thread::sleep(POLL),
    )
}

/// Opens the serial port at `path` as a console transport whose reads return within 100 ms, for
/// [`Client`](crate::Client). The baud rate does not matter on a USB CDC port, and 115200 suits a UART bridge.
/// The port does not borrow `path`, so it can outlive the string that named it:
///
/// ```no_run
/// fn open(board: &mcu_run::Board) -> Result<impl std::io::Read + std::io::Write, mcu_run::Error> {
///     let path = mcu_run::port(board)?;
///     mcu_run::open_port(&path)
/// }
/// ```
pub fn open_port(path: &str) -> Result<impl std::io::Read + std::io::Write + use<>, Error> {
    serialport::new(path, 115_200)
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|error| Error::Transport(format!("{path}: {error}")))
}

/// The processes other than this one that hold the file at `path` open, each as `<command> (pid <n>)`. Empty when
/// none does, and when `lsof` is not installed or fails, so a host without `lsof` keeps working.
pub fn port_holders(path: &str) -> Vec<String> {
    // lsof exits with 1 both when no process holds the file and when the file does not exist, so only its
    // output counts.
    std::process::Command::new("lsof")
        .args(["-w", "-F", "pc", path])
        .output()
        .map(|output| parse_holders(&String::from_utf8_lossy(&output.stdout), std::process::id()))
        .unwrap_or_default()
}

/// Fails with [`Error::PortInUse`] when another process holds the board's console port, before a flash or a reset
/// that would disturb it. A board without a console port passes.
pub(crate) fn check_free(board: &Board) -> Result<(), Error> {
    refuse_held(port(board).ok(), port_holders)
}

/// Fails with [`Error::PortInUse`] when `holders` names a process for `port`. `None` passes.
fn refuse_held(
    port: Option<String>,
    holders: impl FnOnce(&str) -> Vec<String>,
) -> Result<(), Error> {
    let Some(port) = port else {
        return Ok(());
    };
    let holders = holders(&port);
    if holders.is_empty() {
        Ok(())
    } else {
        Err(Error::PortInUse { port, holders })
    }
}

/// The holders in `lsof -F pc` output, without the process `own`. A `p<pid>` line starts each process, and its
/// `c<command>` line follows.
fn parse_holders(output: &str, own: u32) -> Vec<String> {
    let mut holders: Vec<(u32, String)> = Vec::new();
    // A `p` line that is not a number makes the lines after it belong to no process.
    let mut process: Option<u32> = None;
    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            process = pid.parse().ok().filter(|&pid| pid != own);
            if let Some(pid) = process {
                holders.push((pid, "?".to_owned()));
            }
        } else if let (Some(command), Some(pid)) = (line.strip_prefix('c'), process)
            && let Some(last) = holders.last_mut().filter(|(last, _)| *last == pid)
        {
            last.1 = command.to_owned();
        }
    }
    holders
        .into_iter()
        .map(|(pid, command)| format!("{command} (pid {pid})"))
        .collect()
}

/// The USB serial ports attached now.
fn ports() -> Result<Vec<PortInfo>, Error> {
    let ports = serialport::available_ports().map_err(|error| Error::Ports(error.to_string()))?;
    Ok(ports
        .into_iter()
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(usb) => Some(PortInfo {
                path: port.port_name,
                serial: usb.serial_number,
            }),
            _ => None,
        })
        .collect())
}

/// The one port whose USB serial number is `serial`, ignoring case. macOS lists each device as `/dev/cu.*` and
/// `/dev/tty.*`, and the `tty` node waits for carrier on open, so only `cu` counts.
fn choose(ports: &[PortInfo], serial: &str) -> Result<String, Error> {
    let matching: Vec<&PortInfo> = ports
        .iter()
        .filter(|port| !port.path.starts_with("/dev/tty."))
        .filter(|port| {
            port.serial
                .as_deref()
                .is_some_and(|found| !found.is_empty() && found.eq_ignore_ascii_case(serial))
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one.path.clone()),
        [] => Err(Error::NoPort {
            serial: serial.to_owned(),
            ports: ports
                .iter()
                .map(|port| {
                    format!(
                        "{}  {}",
                        port.path,
                        port.serial.as_deref().unwrap_or("(no serial number)")
                    )
                })
                .collect(),
        }),
        many => Err(Error::ManyPorts {
            serial: serial.to_owned(),
            paths: many.iter().map(|port| port.path.clone()).collect(),
        }),
    }
}

/// Looks for the port until it is there or `timeout` has passed. `list` gives the ports now, `elapsed` the
/// time since the wait began, and `sleep` waits between looks.
fn wait<L, E, S>(
    serial: &str,
    timeout: Duration,
    mut list: L,
    mut elapsed: E,
    mut sleep: S,
) -> Result<String, Error>
where
    L: FnMut() -> Result<Vec<PortInfo>, Error>,
    E: FnMut() -> Duration,
    S: FnMut(),
{
    loop {
        match choose(&list()?, serial) {
            Err(Error::NoPort { .. }) if elapsed() < timeout => sleep(),
            Err(Error::NoPort { .. }) => {
                return Err(Error::PortGone {
                    serial: serial.to_owned(),
                    waited: timeout,
                });
            }
            result => return result,
        }
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn info(path: &str, serial: Option<&str>) -> PortInfo {
        PortInfo {
            path: path.to_owned(),
            serial: serial.map(str::to_owned),
        }
    }

    fn ports() -> Vec<PortInfo> {
        vec![
            info("/dev/cu.usbmodem101", Some("AA:BB:CC:DD:EE:01")),
            info("/dev/tty.usbmodem101", Some("AA:BB:CC:DD:EE:01")),
            info("/dev/cu.usbmodem1102", Some("0123456789ABCDEF01234567")),
            info("/dev/tty.usbmodem1102", Some("0123456789ABCDEF01234567")),
            info("/dev/cu.Bluetooth-Incoming-Port", None),
        ]
    }

    #[test]
    fn the_serial_is_the_part_of_the_selector_after_vid_and_pid() {
        let board = |selector: &str| Board {
            selector: selector.to_owned(),
            name: String::new(),
            chip: None,
        };
        assert_eq!(
            board("303a:1001:AA:BB:CC:DD:EE:01").serial(),
            "AA:BB:CC:DD:EE:01"
        );
        assert_eq!(board("0483:3754:0123").serial(), "0123");
        assert_eq!(board("0483:3754").serial(), "");
    }

    #[test]
    fn the_cu_port_with_the_serial_number_is_chosen_regardless_of_case() {
        assert_eq!(
            choose(&ports(), "aa:bb:cc:dd:ee:01").unwrap(),
            "/dev/cu.usbmodem101"
        );
        assert_eq!(
            choose(&ports(), "0123456789abcdef01234567").unwrap(),
            "/dev/cu.usbmodem1102"
        );
    }

    #[test]
    fn no_match_lists_every_usb_port_and_an_empty_serial_matches_nothing() {
        let error = choose(&ports(), "12:34:56:78:9A:02").unwrap_err();
        assert_eq!(
            error,
            Error::NoPort {
                serial: "12:34:56:78:9A:02".to_owned(),
                ports: vec![
                    "/dev/cu.usbmodem101  AA:BB:CC:DD:EE:01".to_owned(),
                    "/dev/tty.usbmodem101  AA:BB:CC:DD:EE:01".to_owned(),
                    "/dev/cu.usbmodem1102  0123456789ABCDEF01234567".to_owned(),
                    "/dev/tty.usbmodem1102  0123456789ABCDEF01234567".to_owned(),
                    "/dev/cu.Bluetooth-Incoming-Port  (no serial number)".to_owned(),
                ],
            }
        );
        let text = error.to_string();
        assert!(
            text.starts_with("no serial port has the serial number 12:34:56:78:9A:02."),
            "{text}"
        );
        assert!(
            text.contains("/dev/cu.usbmodem101  AA:BB:CC:DD:EE:01"),
            "{text}"
        );
        assert!(matches!(
            choose(&[info("/dev/ttyACM0", Some(""))], ""),
            Err(Error::NoPort { .. })
        ));
    }

    #[test]
    fn two_ports_with_the_serial_number_are_reported() {
        let ports = [
            info("/dev/ttyACM0", Some("AB")),
            info("/dev/ttyACM1", Some("ab")),
        ];
        let error = choose(&ports, "AB").unwrap_err();
        assert_eq!(
            error,
            Error::ManyPorts {
                serial: "AB".to_owned(),
                paths: vec!["/dev/ttyACM0".to_owned(), "/dev/ttyACM1".to_owned()],
            }
        );
        assert_eq!(
            error.to_string(),
            "2 serial ports have the serial number AB: /dev/ttyACM0, /dev/ttyACM1"
        );
    }

    #[test]
    fn the_wait_looks_again_until_the_port_is_back() {
        let looks = Cell::new(0);
        let slept = Cell::new(0);
        let path = wait(
            "AB",
            Duration::from_secs(5),
            || {
                looks.set(looks.get() + 1);
                Ok(if looks.get() < 3 {
                    vec![]
                } else {
                    vec![info("/dev/ttyACM0", Some("AB"))]
                })
            },
            || Duration::from_millis(250 * slept.get()),
            || slept.set(slept.get() + 1),
        )
        .unwrap();
        assert_eq!(path, "/dev/ttyACM0");
        assert_eq!((looks.get(), slept.get()), (3, 2));
    }

    #[test]
    fn the_wait_gives_up_at_the_deadline() {
        let slept = Cell::new(0u64);
        let error = wait(
            "AB",
            Duration::from_secs(1),
            || Ok(vec![]),
            || Duration::from_millis(250 * slept.get()),
            || {
                // A wait that never gives up would sleep for ever. Fail instead of hanging.
                assert!(slept.get() < 10, "the wait did not stop at the deadline");
                slept.set(slept.get() + 1);
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            Error::PortGone {
                serial: "AB".to_owned(),
                waited: Duration::from_secs(1),
            }
        );
        assert_eq!(slept.get(), 4);
        assert_eq!(
            error.to_string(),
            "the serial port with the serial number AB did not return within 1s"
        );
    }

    #[test]
    fn the_wait_passes_other_failures_through_at_once() {
        let error = wait(
            "AB",
            Duration::from_secs(5),
            || Err(Error::Ports("usb is down".to_owned())),
            || Duration::ZERO,
            || panic!("slept"),
        )
        .unwrap_err();
        assert_eq!(error, Error::Ports("usb is down".to_owned()));
        assert_eq!(
            error.to_string(),
            "could not list the serial ports: usb is down"
        );
    }

    #[test]
    fn lsof_output_gives_each_other_process_with_its_command() {
        let output = "p4321\ncmcu\np99\ncscreen\np7\ncxtask\n";
        assert_eq!(
            parse_holders(output, 7),
            ["mcu (pid 4321)", "screen (pid 99)"]
        );
        assert_eq!(parse_holders("", 7), Vec::<String>::new());
    }

    #[test]
    fn lsof_lines_out_of_order_or_unknown_are_ignored() {
        // A command before any pid, a field this parser does not read, and a pid that is not a number.
        let output = "corphan\nf3\npnot-a-pid\ncbroken\np12\nccat\n";
        assert_eq!(parse_holders(output, 7), ["cat (pid 12)"]);
        // A pid without a command line is a process all the same.
        assert_eq!(parse_holders("p12\n", 7), ["? (pid 12)"]);
    }

    #[test]
    fn a_held_port_is_refused_with_its_holders_and_a_free_or_absent_one_passes() {
        let error = refuse_held(Some("/dev/cu.usbmodem101".to_owned()), |path| {
            assert_eq!(path, "/dev/cu.usbmodem101");
            vec!["mcu (pid 4321)".to_owned()]
        })
        .unwrap_err();
        assert_eq!(
            error,
            Error::PortInUse {
                port: "/dev/cu.usbmodem101".to_owned(),
                holders: vec!["mcu (pid 4321)".to_owned()],
            }
        );
        assert_eq!(
            error.to_string(),
            "/dev/cu.usbmodem101 is held by mcu (pid 4321). Close it, or name another board with -d probe://<part of its selector>"
        );
        assert_eq!(
            refuse_held(Some("/dev/cu.usbmodem101".to_owned()), |_| vec![]),
            Ok(())
        );
        assert_eq!(refuse_held(None, |_| panic!("no port to look up")), Ok(()));
    }

    #[test]
    fn another_process_that_holds_a_file_is_named_and_this_one_is_not() {
        if std::process::Command::new("lsof")
            .arg("-v")
            .output()
            .is_err()
        {
            return;
        }
        let path = std::env::temp_dir().join(format!("mcu-run-held-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let _this_process_holds_it_too = std::fs::File::open(&path).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::fs::File::open(&path).unwrap())
            .spawn()
            .unwrap();
        let holders = port_holders(path.to_str().unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(holders, [format!("sleep (pid {})", child.id())]);
        assert_eq!(port_holders("/no/such/file"), Vec::<String>::new());
    }
}
