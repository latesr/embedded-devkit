//! What the scenario tests share: the board crate that `MCU_BOARD` names, and its examples as `mcu-scenario`
//! firmware.

#![allow(dead_code)]

use mcu_run::{Board, Client, Image, Reply, Runner};
use mcu_scenario::{Console, Firmware};
use std::io::{BufRead, BufReader, Lines, Read, Write};
use std::net::Ipv4Addr;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// How long a quick command may take to answer.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// A board crate, and what its `info` writes on any unit of its board. A unit's own values, such as its ID, chip
/// revision and module memory, are read from the board, never written here.
pub struct BoardCrate {
    /// The board crate's folder, which `cargo xtask hil --board` names.
    pub folder: &'static str,
    /// The board crate's package name.
    pub package: &'static str,
    /// Every feature of the board crate. `cargo xtask hil` builds the examples with all of them, so the scenario
    /// tests build with the same set and find the examples already built.
    features: &'static [&'static str],
    /// The name the examples give the console, which its banner and prompt show.
    pub prompt: &'static str,
    /// The `board:` line of `info`.
    pub board: &'static str,
    /// The chip's name, which the `chip:` line of `info` writes before the revision.
    pub chip: &'static str,
    /// The `cpu:` line of `info`.
    pub cpu: &'static str,
    /// What the `id:` line of `info` holds.
    pub id: Id,
    /// What the board crate adds to the ID to make the Ethernet MAC, or `None` for a board without Ethernet.
    pub ethernet_mac_offset: Option<u64>,
    /// Whether the board has Wi-Fi and its crate a `console_wifi` example.
    pub wifi: bool,
}

/// What a board's `info` writes as its ID.
pub enum Id {
    /// The chip's MAC. The board's probe gives the same MAC as its USB serial number.
    UsbSerial,
    /// A unique ID of this many hex digits, which only the chip holds.
    Hex(usize),
}

const BOARD_CRATES: [BoardCrate; 3] = [
    BoardCrate {
        folder: "esp/embedded-devkit-esp32-c5-devkitc-1",
        package: "embedded-devkit-esp32-c5-devkitc-1",
        features: &["embassy", "wifi", "iperf"],
        prompt: "c5",
        board: "board: esp32-c5-devkitc-1",
        chip: "esp32c5",
        cpu: "cpu: 240 MHz",
        id: Id::UsbSerial,
        ethernet_mac_offset: None,
        wifi: true,
    },
    BoardCrate {
        folder: "esp/embedded-devkit-esp32-s31-function-coreboard-1",
        package: "embedded-devkit-esp32-s31-function-coreboard-1",
        features: &["embassy", "wifi", "iperf", "ethernet", "perf"],
        prompt: "s31",
        board: "board: esp32-s31-function-coreboard-1",
        chip: "esp32s31",
        cpu: "cpu: 320 MHz",
        id: Id::UsbSerial,
        // The board crate's ethernet.rs gives Ethernet the chip's second universal address, the base MAC plus 1.
        ethernet_mac_offset: Some(1),
        wifi: true,
    },
    BoardCrate {
        folder: "stm32/embedded-devkit-nucleo-h7s3l8",
        package: "embedded-devkit-nucleo-h7s3l8",
        features: &["embassy"],
        prompt: "nucleo",
        board: "board: nucleo-h7s3l8",
        chip: "STM32H7S3L8",
        cpu: "cpu: 600 MHz",
        // The 96-bit unique device ID.
        id: Id::Hex(24),
        // The NUCLEO's Ethernet comes in a later step.
        ethernet_mac_offset: None,
        wifi: false,
    },
];

/// `mac` plus `offset`, as one 48-bit number, in the form `info` writes: lower-case hex pairs with colons.
pub fn mac_plus(mac: &str, offset: u64) -> String {
    let value = u64::from_str_radix(&mac.replace(':', ""), 16).unwrap() + offset;
    let bytes = value.to_be_bytes();
    bytes[2..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The board crate that `MCU_BOARD` names. `cargo xtask hil` sets it.
pub fn board_crate() -> &'static BoardCrate {
    let folder = std::env::var("MCU_BOARD").expect("MCU_BOARD is not set: run cargo xtask hil");
    BOARD_CRATES
        .iter()
        .find(|board| board.folder == folder)
        .unwrap_or_else(|| panic!("MCU_BOARD names {folder}, which the scenario tests do not know"))
}

impl BoardCrate {
    /// The `firmware` line of `info` for one of the board crate's examples.
    pub fn firmware(&self) -> String {
        format!("firmware: {} 0.1.0", self.package)
    }
}

/// The value of the body line `key: value`.
pub fn field<'a>(reply: &'a Reply, key: &str) -> &'a str {
    reply
        .field(key)
        .unwrap_or_else(|| panic!("no {key} line in {:?}", reply.body))
}

/// `ping`'s default payload, in bytes.
pub const PING_PAYLOAD: usize = 56;

/// Sends `count` pings of `payload` bytes 0.2 s apart from this host to `address` with the system's `ping`, and
/// returns how many replies came back.
pub fn ping_from_host(address: &str, count: u32, payload: usize) -> u32 {
    let output = Command::new("ping")
        .args([
            "-q",
            "-c",
            &count.to_string(),
            "-i",
            "0.2",
            "-s",
            &payload.to_string(),
            address,
        ])
        .output()
        .unwrap();
    let summary = String::from_utf8_lossy(&output.stdout);
    // macOS writes "5 packets transmitted, 5 packets received", and Linux "5 packets transmitted, 5 received".
    summary
        .split("transmitted, ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("no summary from ping: {summary}"))
}

/// The board crate's `example`, built in the board crate's folder with every feature.
pub fn firmware(example: &'static str) -> Firmware {
    static DIR: OnceLock<&'static str> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = format!("{}/../{}", env!("CARGO_MANIFEST_DIR"), board_crate().folder);
        Box::leak(dir.into_boxed_str())
    });
    Firmware::example(example)
        .in_dir(dir)
        .features(board_crate().features)
}

/// The function and the source file of each frame at `address` in the board crate's `example`, innermost first,
/// from the image's debug info. A frame inlined into another is its own entry.
pub fn frames_at(example: &'static str, address: u64) -> Vec<(String, String)> {
    let loader = addr2line::Loader::new(firmware(example).elf_path().unwrap()).unwrap();
    let mut frames = loader.find_frames(address).unwrap();
    let mut found = Vec::new();
    while let Some(frame) = frames.next().unwrap() {
        let function = frame
            .function
            .map(|name| name.demangle().unwrap().into_owned())
            .unwrap_or_default();
        let file = frame
            .location
            .and_then(|location| location.file)
            .unwrap_or_default()
            .to_owned();
        found.push((function, file));
    }
    found
}

/// The board that `mcu-scenario` runs the board crate's examples on, for the tests that read its USB serial
/// number: the one `MCU_DEVICE` names, or else the one attached with the board crate's chip.
pub fn board() -> Board {
    let elf = firmware("console").elf_path().unwrap();
    let device = std::env::var("MCU_DEVICE")
        .ok()
        .filter(|device| !device.is_empty());
    Runner::new(Image::from_elf(elf).unwrap())
        .unwrap()
        .board(device.as_deref())
        .unwrap()
}

/// A console on the board crate's `example`, just after it starts.
pub fn console(example: &'static str) -> Console {
    firmware(example).console().unwrap()
}

/// A console on the board crate's `example`, flashed and started even when it already runs. The first boot after
/// a flash follows a probe reset, which `info` names `debugger`.
pub fn fresh_console(example: &'static str) -> Console {
    firmware(example).fresh_console().unwrap()
}

/// The address of this host on `board`'s subnet, wired over Wi-Fi. Prints the interface it chose.
pub fn host_toward(board: Ipv4Addr) -> Ipv4Addr {
    let host = mcu_scenario::host_address(board).unwrap();
    eprintln!("host: {host}");
    host.address
}

/// The console's wait for `net iperf`. `cargo xtask iperf-server` gives up after the same time.
const IPERF_LIMIT: Duration = Duration::from_secs(60);

/// The transport of the host's iperf server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Tcp,
    Udp,
}

/// `cargo xtask iperf-server`, the host end of one iperf test, stopped when dropped. It runs from the xtask that
/// `cargo xtask hil` builds, at a path that keeps its one entry in the Mac's application firewall. A test binary
/// takes a new path at each build, and the firewall holds back the start of each new flow into it.
struct HostServer {
    port: u16,
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
}

impl HostServer {
    /// Starts the server for `transport` on a free port, and waits until it listens.
    fn start(transport: Transport) -> Self {
        let xtask = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/xtask");
        let mut command = Command::new(&xtask);
        command.args(["iperf-server", "-p", "0"]);
        if transport == Transport::Udp {
            command.arg("-u");
        }
        let mut child = command
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("{}: {error}: run cargo xtask hil", xtask.display()));
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let listening = lines.next().unwrap().unwrap();
        let port = listening
            .strip_prefix("listening: ")
            .unwrap_or_else(|| panic!("{listening:?} is not the server's first line"))
            .parse()
            .unwrap();
        Self { port, child, lines }
    }

    /// The bytes that the server's final report counts, once it exits.
    fn bytes(mut self) -> u64 {
        let result = self
            .lines
            .by_ref()
            .map_while(Result::ok)
            .find(|line| line.starts_with("result: "))
            .expect("the host's iperf server did not finish its test");
        eprintln!("the host's iperf server: {result}");
        assert!(self.child.wait().unwrap().success());
        let (_, rest) = result.split_once(" us ").unwrap();
        rest.split_once(" bytes").unwrap().0.parse().unwrap()
    }
}

impl Drop for HostServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The byte count, the rate in Mbit/s and the rest of an `interval`, `result` or `server` value of `net iperf`.
pub fn iperf_measure(text: &str) -> (u64, f64, &str) {
    let (_, rest) = text.split_once(" s ").unwrap();
    let (bytes, rest) = rest.split_once(" bytes ").unwrap();
    let (rate, rest) = rest.split_once(" Mbit/s").unwrap();
    (bytes.parse().unwrap(), rate.parse().unwrap(), rest)
}

/// The lost and sent datagram counts of a UDP receiver's `lost <n>/<n>`.
pub fn iperf_lost(rest: &str) -> (u64, u64) {
    let counts = rest
        .strip_prefix(" lost ")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let (lost, sent) = counts.split_once('/').unwrap();
    (lost.parse().unwrap(), sent.parse().unwrap())
}

/// Runs `net iperf <host>:<port> <options>` on the board at `board` against a new `cargo xtask iperf-server` on
/// a free port of this host, and returns the board's reply and the bytes the server's final report counts.
/// Asserts that the command succeeded and that the server finished its test.
pub fn iperf_against_host(
    client: &mut Client<impl Read + Write>,
    board: Ipv4Addr,
    transport: Transport,
    options: &str,
) -> (Reply, u64) {
    let server = HostServer::start(transport);
    let line = format!("net iperf {}:{} {options}", host_toward(board), server.port);
    let reply = client.send(&line, IPERF_LIMIT).unwrap();
    eprintln!("{line}: {:?}", reply.body);
    assert_eq!(reply.result, mcu_run::CommandResult::Ok, "{:?}", reply.body);
    (reply, server.bytes())
}
