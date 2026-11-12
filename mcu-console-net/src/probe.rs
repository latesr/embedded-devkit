//! Every call into embassy-net, so that a trait over IP stacks, if one is ever needed, wraps this file alone.

use embassy_net::dns::DnsQueryType;
use embassy_net::icmp::PacketMetadata;
use embassy_net::icmp::ping::{PingManager, PingParams};
use embassy_net::tcp::{ConnectError, TcpSocket};
#[cfg(feature = "iperf")]
use embassy_net::udp::{self, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, Stack};
use embassy_time::{Duration, Instant, WithTimeout};
#[cfg(feature = "iperf")]
use embedded_iperf2::embassy;
#[cfg(feature = "iperf")]
use embedded_iperf2::{Client, Report, TestError, Transport};

/// How long the DHCP client waits for an answer to a DISCOVER before it sends another (N-16). smoltcp waits 10 s.
/// A server that starts with the link, as on a direct cable to a host, misses the first DISCOVER.
const DISCOVER_RETRY: smoltcp::time::Duration = smoltcp::time::Duration::from_secs(2);

/// The stack configuration for DHCP that a board crate passes to `embassy_net::new`. The client sends its
/// DISCOVER again 2 s after the last, until a server answers (N-16). Everything else is embassy-net's default.
pub fn dhcp_config() -> embassy_net::Config {
    let mut dhcp = embassy_net::DhcpConfig::default();
    dhcp.retry_config.discover_timeout = DISCOVER_RETRY;
    embassy_net::Config::dhcpv4(dhcp)
}

/// What `net config` reads from the stack at one moment.
pub(crate) struct Snapshot {
    pub(crate) mac: [u8; 6],
    pub(crate) link_up: bool,
    pub(crate) address: Option<Ipv4Cidr>,
    pub(crate) gateway: Option<Ipv4Address>,
    /// The resolvers, first `dns_count` of them.
    pub(crate) dns: [Ipv4Address; 3],
    pub(crate) dns_count: usize,
}

pub(crate) fn snapshot(stack: Stack<'_>) -> Snapshot {
    let mac = match stack.hardware_address() {
        embassy_net::HardwareAddress::Ethernet(address) => address.0,
        #[allow(unreachable_patterns)]
        _ => [0; 6],
    };
    let config = stack.config_v4();
    let mut dns = [Ipv4Address::UNSPECIFIED; 3];
    let mut dns_count = 0usize;
    if let Some(config) = &config {
        for (slot, server) in dns.iter_mut().zip(config.dns_servers.iter()) {
            *slot = *server;
            dns_count = dns_count.saturating_add(1);
        }
    }
    Snapshot {
        mac,
        link_up: stack.is_link_up(),
        address: config.as_ref().map(|config| config.address),
        gateway: config.and_then(|config| config.gateway),
        dns,
        dns_count,
    }
}

/// The IPv4 address the stack holds now.
pub(crate) fn address(stack: Stack<'_>) -> Option<Ipv4Cidr> {
    stack.config_v4().map(|config| config.address)
}

/// Waits up to `timeout` for the stack to hold an IPv4 address.
pub(crate) async fn wait_for_address(stack: Stack<'_>, timeout: Duration) -> Option<Ipv4Cidr> {
    stack.wait_config_up().with_timeout(timeout).await.ok()?;
    address(stack)
}

/// The A records for `name`, or `name` itself when it is an IPv4 address. Calls `found` for each.
pub(crate) async fn lookup(
    stack: Stack<'_>,
    name: &str,
    found: &mut impl FnMut(Ipv4Address),
) -> Result<(), &'static str> {
    let addresses = stack
        .dns_query(name, DnsQueryType::A)
        .await
        .map_err(|error| match error {
            embassy_net::dns::Error::InvalidName | embassy_net::dns::Error::NameTooLong => {
                "not a valid name"
            }
            embassy_net::dns::Error::Failed => "lookup failed",
        })?;
    let mut any = false;
    for address in addresses {
        let IpAddress::Ipv4(address) = address;
        found(address);
        any = true;
    }
    if any { Ok(()) } else { Err("lookup failed") }
}

/// The first A record for `host`, or `host` itself when it is an IPv4 address.
pub(crate) async fn resolve(stack: Stack<'_>, host: &str) -> Result<Ipv4Address, &'static str> {
    let mut first = None;
    lookup(stack, host, &mut |address| {
        first.get_or_insert(address);
    })
    .await?;
    first.ok_or("lookup failed")
}

/// Sends one echo request to `target` and returns its round trip, or `None` when no reply came within `timeout`.
pub(crate) async fn ping(
    stack: Stack<'_>,
    target: Ipv4Address,
    timeout: Duration,
) -> Option<Duration> {
    let mut rx_meta = [PacketMetadata::EMPTY; 1];
    let mut rx = [0; 128];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx = [0; 128];
    let mut manager = PingManager::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    let mut params = PingParams::new(target);
    params
        .set_count(1)
        .set_timeout(timeout)
        .set_rate_limit(Duration::from_ticks(0));
    manager.ping(&params).await.ok()
}

/// How one TCP connect ended.
pub(crate) enum Connect {
    /// The handshake completed after this long.
    Opened(Duration),
    /// The far end answered with a RST.
    Refused,
    /// Nothing answered in time.
    TimedOut,
}

/// How many datagrams the UDP socket of `net iperf` queues each way. 32 datagrams of 1470 bytes fill a 46 KiB
/// buffer, so a larger send buffer gains nothing.
#[cfg(feature = "iperf")]
const IPERF_DATAGRAMS: usize = 32;

/// Runs the test of `client` against `server`, over a TCP or UDP socket in `rx` and `tx`, and returns how it
/// ended. `on_report` receives each interval and the final report. An error is the reason the console writes.
#[cfg(feature = "iperf")]
pub(crate) async fn iperf(
    stack: Stack<'_>,
    rx: &mut [u8],
    tx: &mut [u8],
    client: &mut Client<'_>,
    server: IpEndpoint,
    on_report: impl FnMut(&Report),
) -> Result<Result<(), TestError>, &'static str> {
    let outcome = match client.config().transport {
        Transport::Tcp => {
            let mut socket = TcpSocket::new(stack, rx, tx);
            embassy::run_tcp_client(client, &mut socket, server, on_report).await
        }
        Transport::Udp => {
            let mut rx_meta = [udp::PacketMetadata::EMPTY; IPERF_DATAGRAMS];
            let mut tx_meta = [udp::PacketMetadata::EMPTY; IPERF_DATAGRAMS];
            let mut socket = UdpSocket::new(stack, &mut rx_meta, rx, &mut tx_meta, tx);
            socket.bind(0).map_err(|_| "the socket did not bind")?;
            embassy::run_udp_client(client, &mut socket, server, on_report).await
        }
    };
    outcome.map_err(|error| match error {
        embassy::Error::Connect(ConnectError::ConnectionReset) => {
            "the server refused the connection"
        }
        embassy::Error::Connect(ConnectError::TimedOut) => "the connection timed out",
        // The socket is new and a client never accepts, so neither can happen.
        embassy::Error::Connect(ConnectError::InvalidState) | embassy::Error::Accept(_) => {
            "the connection failed"
        }
        embassy::Error::Connect(ConnectError::NoRoute)
        | embassy::Error::Send(udp::SendError::NoRoute) => "no route to the host",
        embassy::Error::Send(_) => "a send failed",
        embassy::Error::SendTimedOut => "a send timed out",
        embassy::Error::Mismatch => "-l is longer than the socket buffer",
    })
}

/// Opens a TCP connection to `to` and aborts it with a RST once it opens. Waits for the RST to go out before the
/// socket is dropped, because a dropped socket sends nothing.
pub(crate) async fn connect(stack: Stack<'_>, to: IpEndpoint, timeout: Duration) -> Connect {
    let mut rx = [0; 64];
    let mut tx = [0; 64];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    let start = Instant::now();
    let outcome = match socket.connect(to).with_timeout(timeout).await {
        Ok(Ok(())) => Connect::Opened(start.elapsed()),
        Ok(Err(ConnectError::ConnectionReset)) => Connect::Refused,
        Ok(Err(_)) | Err(_) => Connect::TimedOut,
    };
    socket.abort();
    let _ = socket.flush().with_timeout(timeout).await;
    outcome
}
