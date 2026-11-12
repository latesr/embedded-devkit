//! The `net` command set.

#[cfg(feature = "iperf")]
use crate::iperf;
use crate::probe::{self, Connect};
use crate::wifi::{AccessPoint, Network, Ssid, Wifi};
use core::fmt::{self, Display, Formatter, Write as _};
use embassy_futures::select::{Either, select};
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Instant, Timer, WithTimeout};
use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};

/// How many probes `net ping` or `net latency` sends, how far apart they start, and how long each waits for its
/// answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Probes {
    /// How many probes to send, at most 64. A larger count sends 64.
    pub count: u16,
    /// The time from the start of one probe to the start of the next, in milliseconds.
    pub interval_ms: u32,
    /// How long one probe waits for its answer, in milliseconds.
    pub timeout_ms: u32,
}

impl Probes {
    /// `net ping`'s probes: 5 echo requests 0.5 s apart, each waiting up to 1 s (N-4).
    pub const PING: Probes = Probes {
        count: 5,
        interval_ms: 500,
        timeout_ms: 1_000,
    };

    /// `net latency`'s probes: 20 connects 0.5 s apart, each waiting up to 3 s (N-5).
    pub const CONNECT: Probes = Probes {
        count: 20,
        interval_ms: 500,
        timeout_ms: 3_000,
    };

    fn count(self) -> usize {
        usize::from(self.count).min(MAX_PROBES)
    }

    fn timeout(self) -> Duration {
        Duration::from_millis(u64::from(self.timeout_ms))
    }

    /// When probe `index` starts, counted from `start`.
    fn start_of(self, start: Instant, index: usize) -> Instant {
        let offset = u64::from(self.interval_ms).saturating_mul(index as u64);
        start.saturating_add(Duration::from_millis(offset))
    }
}

/// The most probes one command sends, which bounds the samples it keeps for the percentiles.
const MAX_PROBES: usize = 64;

/// How many scan rows `net wifi use` can choose from.
const ROWS: usize = 16;

/// How long `net wifi join` waits for the radio to join (N-11).
const JOIN_TIMEOUT: Duration = Duration::from_secs(20);

/// How long `net wifi join` waits for an address once joined (N-11).
const ADDRESS_WAIT: Duration = Duration::from_secs(30);

/// The port `net latency` connects to when the line names none (N-5).
const LATENCY_PORT: u16 = 443;

#[cfg(feature = "iperf")]
const USAGE: &str = "usage: net config|dns|ping|latency|iperf|wifi";
#[cfg(not(feature = "iperf"))]
const USAGE: &str = "usage: net config|dns|ping|latency|wifi";
const WIFI_USAGE: &str = "usage: net wifi scan|use|ssid|bssid|password|join|drop";

/// The `help` lines of the IP commands. `help` lists them first, then `net iperf` with the `iperf` feature, then
/// the Wi-Fi commands, which a board on Ethernet leaves out.
static IP_HELP: [Help; 4] = [
    Help {
        usage: "net config",
        summary: "the medium, the address and the Wi-Fi network",
        secret: false,
    },
    Help {
        usage: "net dns <name>",
        summary: "the A records for a name",
        secret: false,
    },
    Help {
        usage: "net ping <host> [count]",
        summary: "the round trip of echo requests, until the count or Ctrl-C",
        secret: false,
    },
    Help {
        usage: "net latency <host[:port]>",
        summary: "the time of TCP connects, 443 by default",
        secret: false,
    },
];

#[cfg(feature = "iperf")]
static IPERF_HELP: [Help; 1] = [Help {
    usage: "net iperf <host[:port]> [-u] [-R] [-t s|-n bytes] [-l len] [-b rate] [-i s]",
    summary: "an iperf 2 test against a server, 5001 by default",
    secret: false,
}];
#[cfg(not(feature = "iperf"))]
static IPERF_HELP: [Help; 0] = [];

static WIFI_HELP: [Help; 7] = [
    Help {
        usage: "net wifi scan",
        summary: "the access points in range, strongest first",
        secret: false,
    },
    Help {
        usage: "net wifi use <n>",
        summary: "take the ssid and bssid from scan row n",
        secret: false,
    },
    Help {
        usage: "net wifi ssid <ssid>",
        summary: "set the network name and clear the bssid",
        secret: false,
    },
    Help {
        usage: "net wifi bssid <aa:bb:cc:dd:ee:ff|any>",
        summary: "join only this access point, or any",
        secret: false,
    },
    Help {
        usage: "net wifi password <password>",
        summary: "set the password",
        secret: true,
    },
    Help {
        usage: "net wifi join",
        summary: "join the network and wait for an address",
        secret: false,
    },
    Help {
        usage: "net wifi drop",
        summary: "drop the link, and the station joins again",
        secret: false,
    },
];

/// The `net` commands, over `stack` and, on Wi-Fi, the station `W`.
pub struct NetCommands<'d, W> {
    stack: Stack<'d>,
    medium: &'static str,
    mtu: usize,
    wifi: W,
    has_wifi: bool,
    ping: Probes,
    connect: Probes,
    ssid: Option<Ssid>,
    bssid: Option<[u8; 6]>,
    password: [u8; 64],
    password_len: usize,
    rows: [Option<AccessPoint>; ROWS],
    #[cfg(feature = "iperf")]
    iperf: Option<iperf::Buffers<'d>>,
}

impl<'d> NetCommands<'d, ()> {
    /// The `net` commands for a board on Ethernet, without the `net wifi` commands. `mtu` is the largest frame
    /// the driver takes, headers included, which the stack does not report.
    pub fn ethernet(stack: Stack<'d>, mtu: usize) -> Self {
        Self::new(stack, "ethernet", mtu, (), false)
    }
}

impl<'d, W: Wifi> NetCommands<'d, W> {
    /// The `net` commands for a board on Wi-Fi, with `wifi` for the `net wifi` commands. `mtu` is the largest
    /// frame the driver takes, headers included, which the stack does not report.
    pub fn wifi(stack: Stack<'d>, mtu: usize, wifi: W) -> Self {
        Self::new(stack, "wifi", mtu, wifi, true)
    }

    /// Replaces the probes of `net ping` and `net latency`, which start as [`Probes::PING`] and
    /// [`Probes::CONNECT`].
    pub fn with_probes(self, ping: Probes, connect: Probes) -> Self {
        Self {
            ping,
            connect,
            ..self
        }
    }

    /// Gives `net iperf` its buffers: `rx` and `tx` for the socket, and `work` for the client, which builds each
    /// TCP chunk and UDP datagram there. A UDP test's `-l` must fit in `tx` and in `work`. A TCP chunk is at most
    /// `work`, whatever `-l` asks. Without this call, `net iperf` fails with `no iperf buffers`.
    #[cfg(feature = "iperf")]
    pub fn with_iperf(self, rx: &'d mut [u8], tx: &'d mut [u8], work: &'d mut [u8]) -> Self {
        Self {
            iperf: Some(iperf::Buffers { rx, tx, work }),
            ..self
        }
    }

    fn new(stack: Stack<'d>, medium: &'static str, mtu: usize, wifi: W, has_wifi: bool) -> Self {
        Self {
            stack,
            medium,
            mtu,
            wifi,
            has_wifi,
            ping: Probes::PING,
            connect: Probes::CONNECT,
            ssid: None,
            bssid: None,
            password: [0; 64],
            password_len: 0,
            rows: [None; ROWS],
            #[cfg(feature = "iperf")]
            iperf: None,
        }
    }

    async fn config<R: Reply>(&mut self, reply: &mut R) -> CommandResult {
        let snapshot = probe::snapshot(self.stack);
        reply.field("medium", self.medium).await;
        reply.field("mac", Mac(snapshot.mac)).await;
        reply
            .field("link", if snapshot.link_up { "up" } else { "down" })
            .await;
        reply.field("address", OrNone(snapshot.address)).await;
        reply.field("gateway", OrNone(snapshot.gateway)).await;
        let dns = snapshot.dns.get(..snapshot.dns_count).unwrap_or(&[]);
        if dns.is_empty() {
            reply.field("dns", "none").await;
        } else {
            reply.field("dns", List(dns)).await;
        }
        reply.field("mtu", self.mtu).await;
        if self.has_wifi {
            let joined = self.wifi.status();
            reply.field("ssid", OrNone(joined.map(|ap| ap.ssid))).await;
            reply
                .field("bssid", OrNone(joined.map(|ap| Mac(ap.bssid))))
                .await;
            reply
                .field("channel", OrNone(joined.map(|ap| ap.channel)))
                .await;
            reply
                .field("rssi", OrNone(joined.map(|ap| Dbm(ap.rssi))))
                .await;
        }
        CommandResult::Ok
    }

    async fn dns<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        let Some(name) = args.next() else {
            return CommandResult::Err("usage: net dns <name>");
        };
        let mut found = [embassy_net::Ipv4Address::UNSPECIFIED; 4];
        let mut count = 0usize;
        let looked_up = probe::lookup(self.stack, name, &mut |address| {
            if let Some(slot) = found.get_mut(count) {
                *slot = address;
                count = count.saturating_add(1);
            }
        })
        .await;
        if let Err(reason) = looked_up {
            return CommandResult::Err(reason);
        }
        for address in found.get(..count).unwrap_or(&[]) {
            reply.field("address", address).await;
        }
        CommandResult::Ok
    }

    /// Stops between probes when asked, so each probe it counts as sent has had its full wait.
    async fn ping<R: Reply>(
        &mut self,
        mut args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        let usage = CommandResult::Err("usage: net ping <host> [count]");
        let Some(host) = args.next() else {
            return usage;
        };
        let count = match args.next() {
            None => self.ping.count(),
            Some(count) => match count.parse::<usize>() {
                Ok(count) if count > 0 => count,
                _ => return usage,
            },
        };
        let target = match probe::resolve(self.stack, host).await {
            Ok(target) => target,
            Err(reason) => return CommandResult::Err(reason),
        };
        let probes = self.ping;
        let mut times = RoundTrips::default();
        let mut sent = 0usize;
        let start = Instant::now();
        while sent < count {
            let next = Timer::at(probes.start_of(start, sent));
            if let Either::Second(()) = select(next, cancel.requested()).await {
                break;
            }
            if let Some(round_trip) = probe::ping(self.stack, target, probes.timeout()).await {
                times.push(round_trip);
            }
            sent = sent.saturating_add(1);
        }
        reply
            .field(
                "ping",
                format_args!("sent {sent} received {}{}", times.len, times.min_avg_max()),
            )
            .await;
        if times.len == 0 {
            CommandResult::Err("no reply")
        } else {
            CommandResult::Ok
        }
    }

    async fn latency<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        let usage = CommandResult::Err("usage: net latency <host[:port]>");
        let Some(target) = args.next() else {
            return usage;
        };
        let (host, port) = match target.rsplit_once(':') {
            Some((host, port)) => match port.parse() {
                Ok(port) => (host, port),
                Err(_) => return usage,
            },
            None => (target, LATENCY_PORT),
        };
        let address = match probe::resolve(self.stack, host).await {
            Ok(address) => address,
            Err(reason) => return CommandResult::Err(reason),
        };
        reply
            .field("address", format_args!("{address}:{port}"))
            .await;
        let probes = self.connect;
        let (mut times, mut refused, mut timeout) = (Times::default(), 0usize, 0usize);
        let start = Instant::now();
        for index in 0..probes.count() {
            Timer::at(probes.start_of(start, index)).await;
            match probe::connect(
                self.stack,
                IpEndpoint::new(address.into(), port),
                probes.timeout(),
            )
            .await
            {
                Connect::Opened(time) => times.push(time),
                Connect::Refused => refused = refused.saturating_add(1),
                Connect::TimedOut => timeout = timeout.saturating_add(1),
            }
        }
        reply
            .field(
                "connect",
                format_args!(
                    "sent {} opened {} refused {refused} timeout {timeout}{}",
                    probes.count(),
                    times.len,
                    times.spread()
                ),
            )
            .await;
        if times.len == 0 {
            CommandResult::Err("no connection opened")
        } else {
            CommandResult::Ok
        }
    }

    async fn wifi_command<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        if !self.has_wifi {
            return CommandResult::Err("no wifi on this device");
        }
        match args.next() {
            Some("scan") => self.scan(reply).await,
            Some("use") => self.use_row(args, reply).await,
            Some("ssid") => self.set_ssid(args, reply).await,
            Some("bssid") => self.set_bssid(args, reply).await,
            Some("password") => self.set_password(args),
            Some("join") => self.join(reply).await,
            Some("drop") => match self.wifi.drop_link().await {
                Ok(()) => CommandResult::Ok,
                Err(error) => CommandResult::Err(error.reason()),
            },
            _ => CommandResult::Err(WIFI_USAGE),
        }
    }

    async fn scan<R: Reply>(&mut self, reply: &mut R) -> CommandResult {
        self.rows = [None; ROWS];
        let rows = &mut self.rows;
        if let Err(error) = self
            .wifi
            .scan(&mut |found| keep_strongest(rows, found))
            .await
        {
            return CommandResult::Err(error.reason());
        }
        for (index, row) in self.rows.iter().flatten().enumerate() {
            let mut number = Text::<4>::default();
            let _ = write!(number, "{}", index.saturating_add(1));
            reply
                .field(
                    number.as_str(),
                    format_args!(
                        "{}, {}, channel {}, {}",
                        row.ssid,
                        Mac(row.bssid),
                        row.channel,
                        Dbm(row.rssi)
                    ),
                )
                .await;
        }
        CommandResult::Ok
    }

    async fn use_row<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        let Some(number) = args.next().and_then(|word| word.parse::<usize>().ok()) else {
            return CommandResult::Err("usage: net wifi use <n>");
        };
        let Some(row) = number
            .checked_sub(1)
            .and_then(|index| self.rows.get(index).copied().flatten())
        else {
            return CommandResult::Err("no such row");
        };
        self.ssid = Some(row.ssid);
        self.bssid = Some(row.bssid);
        reply.field("ssid", row.ssid).await;
        reply.field("bssid", Mac(row.bssid)).await;
        CommandResult::Ok
    }

    async fn set_ssid<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        let Some(text) = args.rest() else {
            return CommandResult::Err("usage: net wifi ssid <ssid>");
        };
        let Some(ssid) = Ssid::new(text.as_bytes()) else {
            return CommandResult::Err("the ssid is longer than 32 bytes");
        };
        self.ssid = Some(ssid);
        self.bssid = None;
        reply.field("ssid", ssid).await;
        CommandResult::Ok
    }

    async fn set_bssid<R: Reply>(&mut self, mut args: Args<'_>, reply: &mut R) -> CommandResult {
        match args.next() {
            Some("any") => {
                self.bssid = None;
                reply.field("bssid", "any").await;
                CommandResult::Ok
            }
            Some(word) => match parse_mac(word) {
                Some(bssid) => {
                    self.bssid = Some(bssid);
                    reply.field("bssid", Mac(bssid)).await;
                    CommandResult::Ok
                }
                None => CommandResult::Err("usage: net wifi bssid <aa:bb:cc:dd:ee:ff|any>"),
            },
            None => CommandResult::Err("usage: net wifi bssid <aa:bb:cc:dd:ee:ff|any>"),
        }
    }

    fn set_password(&mut self, mut args: Args<'_>) -> CommandResult {
        let password = args.rest().unwrap_or("");
        let Some(slot) = self.password.get_mut(..password.len()) else {
            return CommandResult::Err("the password is longer than 64 bytes");
        };
        slot.copy_from_slice(password.as_bytes());
        self.password_len = password.len();
        CommandResult::Ok
    }

    async fn join<R: Reply>(&mut self, reply: &mut R) -> CommandResult {
        let Some(ssid) = self.ssid else {
            return CommandResult::Err("no ssid");
        };
        let password = self
            .password
            .get(..self.password_len)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("");
        let network = Network {
            ssid: &ssid,
            bssid: self.bssid,
            password,
        };
        match self.wifi.join(&network).with_timeout(JOIN_TIMEOUT).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return CommandResult::Err(error.reason()),
            Err(_) => return CommandResult::Err("the join timed out"),
        }
        let Some(address) = probe::wait_for_address(self.stack, ADDRESS_WAIT).await else {
            return CommandResult::Err("no address yet");
        };
        reply.field("ssid", ssid).await;
        match self.bssid {
            Some(bssid) => reply.field("bssid", Mac(bssid)).await,
            None => reply.field("bssid", "any").await,
        }
        reply.field("address", address).await;
        let carriers = self
            .rows
            .iter()
            .flatten()
            .filter(|row| row.ssid == ssid)
            .count();
        if self.bssid.is_none() && carriers > 1 {
            reply
                .field(
                    "bssids",
                    format_args!("{carriers} for this ssid, see net wifi scan"),
                )
                .await;
        }
        CommandResult::Ok
    }
}

impl<W: Wifi> Commands for NetCommands<'_, W> {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        let wifi: &'static [Help] = if self.has_wifi { &WIFI_HELP } else { &[] };
        IP_HELP.iter().chain(IPERF_HELP.iter()).chain(wifi)
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        mut args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        if name != "net" {
            return CommandResult::Unknown;
        }
        match args.next() {
            Some("config") => self.config(reply).await,
            Some("dns") => self.dns(args, reply).await,
            Some("ping") => self.ping(args, reply, cancel).await,
            Some("latency") => self.latency(args, reply).await,
            #[cfg(feature = "iperf")]
            Some("iperf") => iperf::run(self.stack, self.iperf.as_mut(), args, reply).await,
            Some("wifi") => self.wifi_command(args, reply).await,
            _ => CommandResult::Err(USAGE),
        }
    }
}

/// Puts `found` among `rows`, which stay sorted strongest first, and drops the weakest when they are full.
fn keep_strongest(rows: &mut [Option<AccessPoint>; ROWS], found: AccessPoint) {
    let Some(at) = rows
        .iter()
        .position(|row| row.is_none_or(|row| found.rssi > row.rssi))
    else {
        return;
    };
    if let Some(tail) = rows.get_mut(at..) {
        tail.rotate_right(1);
        if let Some(slot) = tail.first_mut() {
            *slot = Some(found);
        }
    }
}

/// `aa:bb:cc:dd:ee:ff`, in either case.
fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut parts = text.split(':');
    for byte in mac.iter_mut() {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    parts.next().is_none().then_some(mac)
}

/// The round trips that were answered, in microseconds.
struct Times {
    samples: [u64; MAX_PROBES],
    len: usize,
}

impl Default for Times {
    fn default() -> Self {
        Self {
            samples: [0; MAX_PROBES],
            len: 0,
        }
    }
}

impl Times {
    fn push(&mut self, time: Duration) {
        if let Some(slot) = self.samples.get_mut(self.len) {
            *slot = time.as_micros();
            self.len = self.len.saturating_add(1);
        }
    }

    fn sorted(&self) -> [u64; MAX_PROBES] {
        let mut sorted = self.samples;
        if let Some(answered) = sorted.get_mut(..self.len) {
            answered.sort_unstable();
        }
        sorted
    }

    /// ` min <t> median <t> p90 <t> max <t>`, or nothing when no probe was answered.
    fn spread(&self) -> impl Display {
        let sorted = self.sorted();
        let answered = sorted.get(..self.len).unwrap_or(&[]);
        Summary(
            answered
                .first()
                .zip(nearest_rank(answered, 50))
                .zip(nearest_rank(answered, 90))
                .zip(answered.last())
                .map(|(((min, median), p90), max)| {
                    [
                        ("min", *min),
                        ("median", median),
                        ("p90", p90),
                        ("max", *max),
                    ]
                }),
        )
    }
}

/// The `percent`th percentile of `sorted` by the nearest-rank method: the smallest value with at least
/// `percent` in every hundred at or below it. `None` for no values.
fn nearest_rank(sorted: &[u64], percent: usize) -> Option<u64> {
    let rank = percent.saturating_mul(sorted.len()).div_ceil(100).max(1);
    sorted.get(rank.saturating_sub(1)).copied()
}

/// The answered round trips of a `net ping` without a bound on their number: the count, the total, the least
/// and the most, in microseconds.
#[derive(Default)]
struct RoundTrips {
    len: usize,
    total: u64,
    min: u64,
    max: u64,
}

impl RoundTrips {
    fn push(&mut self, time: Duration) {
        let micros = time.as_micros();
        self.min = if self.len == 0 {
            micros
        } else {
            self.min.min(micros)
        };
        self.max = self.max.max(micros);
        self.total = self.total.saturating_add(micros);
        self.len = self.len.saturating_add(1);
    }

    /// ` min <t> avg <t> max <t>`, or nothing when no probe was answered.
    fn min_avg_max(&self) -> impl Display {
        Summary(
            self.total
                .checked_div(self.len as u64)
                .map(|avg| [("min", self.min), ("avg", avg), ("max", self.max)]),
        )
    }
}

/// Labelled times in milliseconds, each written as ` <label> <ms>`.
struct Summary<const N: usize>(Option<[(&'static str, u64); N]>);

impl<const N: usize> Display for Summary<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (label, micros) in self.0.iter().flatten() {
            write!(f, " {label} {}", Millis(*micros))?;
        }
        Ok(())
    }
}

/// Microseconds written as milliseconds with two decimals, such as `10.52`.
pub(crate) struct Millis(pub(crate) u64);

impl Display for Millis {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.0 / 1_000, self.0 % 1_000 / 10)
    }
}

/// A MAC address as `aa:bb:cc:dd:ee:ff`.
#[derive(Clone, Copy)]
struct Mac([u8; 6]);

impl Display for Mac {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// A signal strength as `-41 dBm`.
#[derive(Clone, Copy)]
struct Dbm(i8);

impl Display for Dbm {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} dBm", self.0)
    }
}

/// A value, or `none`.
struct OrNone<T>(Option<T>);

impl<T: Display> Display for OrNone<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("none"),
        }
    }
}

/// Values separated by commas.
struct List<'a, T>(&'a [T]);

impl<T: Display> Display for List<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (index, value) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            value.fmt(f)?;
        }
        Ok(())
    }
}

/// A short text built with `write!`, for a key that is a number. A write past `N` bytes fails.
struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> Text<N> {
    fn as_str(&self) -> &str {
        self.bytes
            .get(..self.len)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }
}

impl<const N: usize> fmt::Write for Text<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len.saturating_add(text.len());
        let slot = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        slot.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_keep_the_least_the_mean_and_the_most_in_any_order() {
        let mut times = RoundTrips::default();
        assert_eq!(times.min_avg_max().to_string(), "");
        for millis in [3, 1, 2] {
            times.push(Duration::from_millis(millis));
        }
        assert_eq!(
            times.min_avg_max().to_string(),
            " min 1.00 avg 2.00 max 3.00"
        );
    }

    #[test]
    fn nearest_rank_takes_the_smallest_value_that_covers_the_percentage() {
        let sorted = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(nearest_rank(&sorted, 50), Some(50));
        assert_eq!(nearest_rank(&sorted, 90), Some(90));
        assert_eq!(nearest_rank(&sorted, 91), Some(100));
        assert_eq!(nearest_rank(&[7, 9, 11], 50), Some(9));
        assert_eq!(nearest_rank(&[7], 90), Some(7));
        assert_eq!(nearest_rank(&[], 50), None);
    }

    #[test]
    fn microseconds_are_written_as_milliseconds_with_two_decimals() {
        assert_eq!(Millis(10_527).to_string(), "10.52");
        assert_eq!(Millis(999).to_string(), "0.99");
        assert_eq!(Millis(3_004_000).to_string(), "3004.00");
    }
}
