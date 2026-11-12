//! Two embassy-net stacks in one process, the device and the far end, joined by a link that holds each frame for
//! [`DELAY`] and records it. The far end answers pings through its own stack, accepts TCP on port 443, and
//! answers DNS queries on port 53. A fake [`Wifi`] stands in for the radio.

#![allow(dead_code)]

use embassy_futures::join::{join, join3};
use embassy_futures::select::{Either, select};
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{Config, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4};
use embassy_net_driver::{Capabilities, Driver, HardwareAddress, LinkState, RxToken, TxToken};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use mcu_console::{Args, CancelSignal, CommandResult, Commands, Reply};
use mcu_console_net::{AccessPoint, Network, Probes, Ssid, Wifi, WifiError};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::task::{Context, Waker};

/// How long the link holds each frame, so a round trip takes at least twice this.
pub const DELAY: Duration = Duration::from_millis(5);
pub const DEVICE: Ipv4Address = Ipv4Address::new(10, 0, 0, 2);
pub const FAR: Ipv4Address = Ipv4Address::new(10, 0, 0, 1);
/// An address nobody on the link holds, so a probe to it is never answered.
pub const NOBODY: Ipv4Address = Ipv4Address::new(10, 0, 0, 9);
pub const DEVICE_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x02];
pub const FAR_MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 0x01];
/// The far end's open TCP port. Every other port refuses.
pub const OPEN_PORT: u16 = 443;

/// Short probes, so a lost one costs milliseconds.
pub const QUICK: Probes = Probes {
    count: 3,
    interval_ms: 30,
    timeout_ms: 200,
};

/// The frames in flight each way, and every frame sent, with the side that sent it: 0 is the device.
#[derive(Default)]
struct Wire {
    queues: [VecDeque<(Instant, Vec<u8>)>; 2],
    wakers: [Option<Waker>; 2],
    frames: Vec<(usize, Vec<u8>)>,
}

struct End {
    wire: Rc<RefCell<Wire>>,
    side: usize,
    mac: [u8; 6],
}

struct Rx(Vec<u8>);

struct Tx {
    wire: Rc<RefCell<Wire>>,
    from: usize,
}

impl RxToken for Rx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R {
        f(&mut self.0)
    }
}

impl TxToken for Tx {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0; len];
        let result = f(&mut frame);
        let to = 1 - self.from;
        let mut wire = self.wire.borrow_mut();
        wire.frames.push((self.from, frame.clone()));
        wire.queues[to].push_back((Instant::now() + DELAY, frame));
        if let Some(waker) = wire.wakers[to].take() {
            waker.wake();
        }
        result
    }
}

impl Driver for End {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx;

    fn receive(&mut self, cx: &mut Context) -> Option<(Rx, Tx)> {
        let mut wire = self.wire.borrow_mut();
        match wire.queues[self.side].front() {
            Some((at, _)) if *at <= Instant::now() => {
                let (_, frame) = wire.queues[self.side].pop_front()?;
                Some((
                    Rx(frame),
                    Tx {
                        wire: self.wire.clone(),
                        from: self.side,
                    },
                ))
            }
            Some((at, _)) => {
                embassy_time_driver::schedule_wake(at.as_ticks(), cx.waker());
                None
            }
            None => {
                wire.wakers[self.side] = Some(cx.waker().clone());
                None
            }
        }
    }

    fn transmit(&mut self, _: &mut Context) -> Option<Tx> {
        Some(Tx {
            wire: self.wire.clone(),
            from: self.side,
        })
    }

    fn link_state(&mut self, _: &mut Context) -> LinkState {
        LinkState::Up
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = 1514;
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.mac)
    }
}

/// The two stacks, and the frames the link carried.
pub struct Net<'a> {
    pub device: Stack<'a>,
    pub far: Stack<'a>,
    wire: Rc<RefCell<Wire>>,
}

impl Net<'_> {
    /// The TCP flags of each frame the link carried to or from the far end's `port`, with the side that sent
    /// it: 0 is the device.
    pub fn tcp_flags(&self, port: u16) -> Vec<(usize, u8)> {
        self.wire
            .borrow()
            .frames
            .iter()
            .filter_map(|(side, frame)| {
                let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
                let ip = frame.get(14..)?;
                if ethertype != 0x0800 || ip[9] != 6 {
                    return None;
                }
                let tcp = ip.get(usize::from(ip[0] & 0x0f) * 4..)?;
                let source = u16::from_be_bytes([tcp[0], tcp[1]]);
                let destination = u16::from_be_bytes([tcp[2], tcp[3]]);
                (source == port || destination == port).then_some((*side, tcp[13]))
            })
            .collect()
    }
}

/// The device's address, gateway and resolver: 10.0.0.2/24, through the far end.
pub fn device_config() -> Config {
    let mut dns_servers = heapless::Vec::new();
    dns_servers.push(FAR).unwrap();
    Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(DEVICE, 24),
        gateway: Some(FAR),
        dns_servers,
    })
}

/// Runs `test` with the two stacks, while both stacks and the far end's services run beside it.
pub fn run<T>(device_config: Config, test: impl AsyncFnOnce(&Net<'_>) -> T) -> T {
    let wire = Rc::new(RefCell::new(Wire::default()));
    let mut device_resources = StackResources::<4>::new();
    let mut far_resources = StackResources::<4>::new();
    let (device, mut device_runner) = embassy_net::new(
        End {
            wire: wire.clone(),
            side: 0,
            mac: DEVICE_MAC,
        },
        device_config,
        &mut device_resources,
        1,
    );
    let (far, mut far_runner) = embassy_net::new(
        End {
            wire: wire.clone(),
            side: 1,
            mac: FAR_MAC,
        },
        Config::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(FAR, 24),
            gateway: None,
            dns_servers: heapless::Vec::new(),
        }),
        &mut far_resources,
        2,
    );
    let net = Net { device, far, wire };
    embassy_futures::block_on(async {
        let background = join3(
            device_runner.run(),
            far_runner.run(),
            embassy_futures::join::join(listen(far), resolve(far)),
        );
        match select(background, test(&net)).await {
            Either::First(_) => unreachable!("the stacks and the services never return"),
            Either::Second(result) => result,
        }
    })
}

/// Accepts one connection at a time on [`OPEN_PORT`], and waits for the device to end it.
async fn listen(stack: Stack<'_>) {
    let mut rx = [0; 256];
    let mut tx = [0; 256];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    loop {
        if socket.accept(OPEN_PORT).await.is_ok() {
            let mut buf = [0; 16];
            while let Ok(n) = socket.read(&mut buf).await {
                if n == 0 {
                    break;
                }
            }
        }
        socket.abort();
        let _ = socket.flush().await;
    }
}

/// The A records the far end's resolver serves. Any other name gets NXDOMAIN.
fn records(name: &str) -> &'static [[u8; 4]] {
    match name {
        "far.test" => &[[10, 0, 0, 1]],
        "two.test" => &[[10, 0, 0, 1], [10, 0, 0, 3]],
        _ => &[],
    }
}

/// Answers each DNS query on port 53 with the A records of [`records`].
async fn resolve(stack: Stack<'_>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx = [0; 512];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx = [0; 512];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    socket.bind(53).unwrap();
    let mut query = [0u8; 512];
    loop {
        let Ok((len, from)) = socket.recv_from(&mut query).await else {
            continue;
        };
        let answer = dns_answer(&query[..len]);
        let _ = socket.send_to(&answer, from.endpoint).await;
    }
}

/// The response to one DNS query: its header and question, then one A record per address.
fn dns_answer(query: &[u8]) -> Vec<u8> {
    let mut at = 12;
    let mut labels = Vec::new();
    while query[at] != 0 {
        let len = usize::from(query[at]);
        labels.push(String::from_utf8_lossy(&query[at + 1..at + 1 + len]).into_owned());
        at += 1 + len;
    }
    let question_end = at + 1 + 4;
    let addresses = records(&labels.join("."));
    let mut answer = query[..question_end].to_vec();
    let rcode = if addresses.is_empty() { 3 } else { 0 };
    answer[2] = 0x81;
    answer[3] = 0x80 | rcode;
    answer[6..8].copy_from_slice(&(addresses.len() as u16).to_be_bytes());
    answer[8..12].fill(0);
    for address in addresses {
        answer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        answer.extend_from_slice(address);
    }
    answer
}

/// Collects a command's body lines.
struct Lines(Vec<String>);

impl Reply for Lines {
    async fn line(&mut self, text: &str) {
        self.0.push(text.to_owned());
    }

    async fn flush(&mut self) {}
}

/// The longest a command may take. Every test's command, with [`QUICK`] probes, finishes well inside it, so a
/// command that ignores its probes fails here and does not hang.
const COMMAND_LIMIT: Duration = Duration::from_secs(5);

/// Runs one console line through `commands`, and returns the body lines and the command's result.
pub async fn send(commands: &mut impl Commands, line: &str) -> (Vec<String>, CommandResult) {
    send_within(commands, line, COMMAND_LIMIT).await
}

/// [`send`], for a command that may take up to `limit`, such as one that waits out a connect timeout.
pub async fn send_within(
    commands: &mut impl Commands,
    line: &str,
    limit: Duration,
) -> (Vec<String>, CommandResult) {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    let mut lines = Lines(Vec::new());
    let cancel = CancelSignal::new();
    let result = commands
        .run(name, Args::new(rest), &mut lines, cancel.cancel())
        .with_timeout(limit)
        .await
        .unwrap_or_else(|_| panic!("{line:?} took longer than {limit:?}"));
    (lines.0, result)
}

/// [`send`], with the command asked to stop once `after` has passed, as a Ctrl-C does.
pub async fn send_and_stop(
    commands: &mut impl Commands,
    line: &str,
    after: Duration,
) -> (Vec<String>, CommandResult) {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    let mut lines = Lines(Vec::new());
    let cancel = CancelSignal::new();
    let stop = async {
        Timer::after(after).await;
        cancel.raise();
    };
    let (result, ()) = join(
        commands
            .run(name, Args::new(rest), &mut lines, cancel.cancel())
            .with_timeout(COMMAND_LIMIT),
        stop,
    )
    .await;
    let result = result.unwrap_or_else(|_| panic!("{line:?} did not stop"));
    (lines.0, result)
}

/// The value of a `key: value` line.
pub fn value<'a>(line: &'a str, key: &str) -> &'a str {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or_else(|| panic!("{line:?} is not a {key} line"))
}

/// The milliseconds after `label` in a probe summary line, such as `min 10.52`.
pub fn millis(line: &str, label: &str) -> f64 {
    let mut words = line.split_whitespace();
    words
        .find(|word| *word == label)
        .unwrap_or_else(|| panic!("{line:?} has no {label}"));
    words.next().unwrap().parse().unwrap()
}

/// What the fake radio holds and what it was asked.
#[derive(Default)]
pub struct Radio {
    pub in_range: Vec<AccessPoint>,
    pub joins: Vec<(Vec<u8>, Option<[u8; 6]>, String)>,
    pub join_error: Option<WifiError>,
    pub joined: Option<AccessPoint>,
    pub drops: usize,
    pub drop_error: Option<WifiError>,
}

/// A [`Wifi`] over a [`Radio`] the test keeps a handle to.
#[derive(Clone, Default)]
pub struct FakeWifi(pub Rc<RefCell<Radio>>);

impl Wifi for FakeWifi {
    async fn scan(&mut self, found: &mut impl FnMut(AccessPoint)) -> Result<(), WifiError> {
        for access_point in self.0.borrow().in_range.iter() {
            found(*access_point);
        }
        Ok(())
    }

    async fn join(&mut self, network: &Network<'_>) -> Result<(), WifiError> {
        let mut radio = self.0.borrow_mut();
        radio.joins.push((
            network.ssid.as_bytes().to_vec(),
            network.bssid,
            network.password.to_owned(),
        ));
        match radio.join_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn drop_link(&mut self) -> Result<(), WifiError> {
        let mut radio = self.0.borrow_mut();
        radio.drops += 1;
        match radio.drop_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn status(&mut self) -> Option<AccessPoint> {
        self.0.borrow().joined
    }
}

/// An access point whose BSSID ends in `last`.
pub fn access_point(ssid: &str, last: u8, channel: u8, rssi: i8) -> AccessPoint {
    AccessPoint {
        ssid: Ssid::new(ssid.as_bytes()).unwrap(),
        bssid: [0xaa, 0xbb, 0xcc, 0, 0, last],
        channel,
        rssi,
    }
}
