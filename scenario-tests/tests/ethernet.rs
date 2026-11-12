//! A board crate's Ethernet, through the `console_ethernet` example and `mcu-console-net`'s commands (B-7, B-20, N-2,
//! N-4, N-5, N-15).
//!
//! The board's Ethernet port must be cabled to a network with a DHCP server, and this host must reach it. A board
//! without Ethernet says so and returns. All probe traffic stays on the local network: pings go to the lease's
//! gateway and from this host to the board, and connects and iperf tests go to a server on this host. The iperf
//! tests' server is embedded-iperf2's, run as `cargo xtask iperf-server`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::{
    PING_PAYLOAD, Transport, board, board_crate, console, field, frames_at, iperf_against_host,
    iperf_lost, iperf_measure, mac_plus, ping_from_host,
};
use mcu_run::{CommandResult, Reply};
use mcu_scenario::Console;
use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::time::Duration;

/// Long enough for 5 pings or 20 connects 0.5 s apart.
const SLOW: Duration = Duration::from_secs(60);

/// The UDP datagram length the 95 Mbit/s and receive tests ask for with `-l`.
const UDP_LEN: u64 = 1470;

/// How long the link and a DHCP lease may take after the firmware starts.
const LEASE: Duration = Duration::from_secs(20);

/// The console of the board's `console_ethernet` example, or `None` for a board without Ethernet.
fn ethernet_console() -> Option<Console> {
    if board_crate().ethernet_mac_offset.is_none() {
        eprintln!("skipped: {} has no Ethernet", board_crate().folder);
        return None;
    }
    Some(console("console_ethernet"))
}

/// Polls `net config` until the link is up and the board holds an address, and returns that reply.
fn leased(console: &mut Console) -> Reply {
    console
        .send_until("net config", LEASE, |config| {
            (field(config, "link") == "up" && field(config, "address") != "none")
                .then(|| config.clone())
        })
        .unwrap()
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn config_reports_the_ethernet_link_its_mac_and_a_lease() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let config = leased(&mut client);
    assert_eq!(field(&config, "medium"), "ethernet");
    let offset = board_crate().ethernet_mac_offset.unwrap();
    let base = board().serial().to_lowercase();
    assert_eq!(field(&config, "mac"), mac_plus(&base, offset));
    assert_eq!(field(&config, "mtu"), "1514");
    assert_ne!(field(&config, "gateway"), "none");
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn ping_reaches_the_gateway_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let gateway = field(&leased(&mut client), "gateway").to_owned();
    let ping = client
        .send_within(&format!("net ping {gateway}"), SLOW)
        .unwrap();
    assert_eq!(ping.result, CommandResult::Ok, "{:?}", ping.body);
    let summary = field(&ping, "ping");
    assert!(summary.starts_with("sent 5 received 5 "), "{summary}");
}

/// Ctrl-C stops a ping that would run for 1000 probes, and it writes what it sent (N-4).
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn ping_stops_at_ctrl_c_and_writes_what_it_sent() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let gateway = field(&leased(&mut client), "gateway").to_owned();
    let ping = client
        .send_interrupted(
            &format!("net ping {gateway} 1000"),
            Duration::from_secs(3),
            SLOW,
        )
        .unwrap();
    assert_eq!(ping.result, CommandResult::Ok, "{:?}", ping.body);
    let summary = field(&ping, "ping");
    let sent: u32 = summary
        .strip_prefix("sent ")
        .and_then(|rest| rest.split(' ').next())
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..1000).contains(&sent), "{summary}");
}

/// The words of a `perf_` line's value that follow each label, as numbers: `size 1 deepest 2` gives `[1, 2]`.
fn labelled(value: &str, labels: &[&str]) -> Vec<u64> {
    let words: Vec<&str> = value.split(' ').collect();
    labels
        .iter()
        .map(|label| {
            let at = words.iter().position(|word| word == label).unwrap();
            words[at + 1].parse().unwrap()
        })
        .collect()
}

/// `perf run` writes the ping's own line, then each figure in its form (P-2 to P-5). The Ethernet example has
/// `perf`. It gives the allocator no memory, so it has no heap.
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn perf_run_writes_the_figures_after_the_command_s_own_lines() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let gateway = field(&leased(&mut client), "gateway").to_owned();
    let reply = client
        .send_within(&format!("perf run net ping {gateway}"), SLOW)
        .unwrap();
    assert_eq!(reply.result, CommandResult::Ok, "{:?}", reply.body);
    assert!(
        field(&reply, "ping").starts_with("sent 5 "),
        "{:?}",
        reply.body
    );
    let us: u64 = field(&reply, "perf_us").parse().unwrap();
    let cycles: u64 = field(&reply, "perf_cycles").parse().unwrap();
    assert!(us > 0 && cycles > us, "{:?}", reply.body);
    let (whole, part) = field(&reply, "perf_cpi").split_once('.').unwrap();
    assert!(
        whole.parse::<u64>().is_ok() && part.len() == 2,
        "{:?}",
        reply.body
    );
    let stack = labelled(
        field(&reply, "perf_stack"),
        &["size", "deepest", "headroom"],
    );
    assert!(stack[1] > 0 && stack[1] + stack[2] == stack[0], "{stack:?}");
    assert_eq!(field(&reply, "perf_heap"), "none");
    assert_eq!(field(&reply, "perf_locks"), "none");
}

/// `perf_locks` counts each lock that the example's `lock` command takes through the board's timed mutex, and gives
/// the address of the code that took the longest, which the image's debug info places in the `lock` command (P-7).
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn perf_locks_counts_the_locks_taken_through_the_timed_mutex() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let reply = client.send_within("perf run lock 100", SLOW).unwrap();
    assert_eq!(reply.result, CommandResult::Ok, "{:?}", reply.body);
    let value = field(&reply, "perf_locks");
    let words: Vec<&str> = value.split(' ').collect();
    assert_eq!(
        (words[0], words[2], words[3], words[4]),
        ("longest_us", "count", "100", "mean_us"),
        "{value}"
    );
    let (longest, mean): (f64, f64) = (words[1].parse().unwrap(), words[5].parse().unwrap());
    assert!(longest >= mean, "{value}");
    assert_eq!(words[6], "at", "{value}");
    let caller = words[7].strip_prefix("0x").expect(value);
    let caller = u64::from_str_radix(caller, 16).expect(value);
    // The return address is the instruction after the call, which can start the next line or function.
    let frames = frames_at("console_ethernet", caller - 1);
    assert!(
        frames.iter().any(|(function, file)| function
            .starts_with("<console_ethernet::lock::Lock as mcu_console::Commands>::run")
            && file.ends_with("console_ethernet.rs")),
        "{value}: {frames:#?}"
    );
}

/// smoltcp drops its reply to the first ping while it asks for this host's hardware address, so one ping goes
/// first and is not counted.
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn the_board_answers_pings_from_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let config = leased(&mut client);
    let board = field(&config, "address").split('/').next().unwrap();
    ping_from_host(board, 1, PING_PAYLOAD);
    assert_eq!(ping_from_host(board, 5, PING_PAYLOAD), 5);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn latency_times_connects_to_a_listener_on_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board: Ipv4Addr = field(&leased(&mut client), "address")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let route = UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect((board, 9)).unwrap();
    let host = route.local_addr().unwrap().ip();
    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || for _ in listener.incoming() {});
    let latency = client
        .send_within(&format!("net latency {host}:{port}"), SLOW)
        .unwrap();
    assert_eq!(latency.result, CommandResult::Ok, "{:?}", latency.body);
    let connect = field(&latency, "connect");
    assert!(connect.starts_with("sent 20 opened 20 "), "{connect}");
}

/// The board's address, from a lease.
fn board_address(client: &mut Console) -> Ipv4Addr {
    field(&leased(client), "address")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_sends_tcp_to_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Tcp, "-t 5 -i 1");
    let intervals = reply
        .body
        .iter()
        .filter(|line| line.starts_with("interval: "))
        .count();
    assert!(intervals >= 5, "{:?}", reply.body);
    let (bytes, rate, _) = iperf_measure(field(&reply, "result"));
    assert_eq!(bytes, server);
    assert!(rate > 1.0, "{:?}", reply.body);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_receives_tcp_from_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Tcp, "-R -t 5");
    let (bytes, rate, _) = iperf_measure(field(&reply, "result"));
    assert_eq!(bytes, server);
    assert!(rate > 1.0, "{:?}", reply.body);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_sends_udp_to_this_host_over_ethernet_without_loss() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Udp, "-u -t 5 -b 10m");
    let (bytes, _, rest) = iperf_measure(field(&reply, "server"));
    assert_eq!(bytes, server);
    assert!(bytes > 5 * 10_000_000 / 8 * 95 / 100, "{:?}", reply.body);
    let (lost, sent) = iperf_lost(rest);
    assert_eq!(lost, 0, "{:?}", reply.body);
    assert!(sent > 4000, "{:?}", reply.body);
}

/// Reports the rate and the loss at 95 Mbit/s in the printed reply. The path may drop datagrams at this rate
/// without a fault on the board, so only the server's count is checked.
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_sends_udp_at_95_mbit_to_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let options = format!("-u -t 5 -b 95m -l {UDP_LEN}");
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Udp, &options);
    let (bytes, _, rest) = iperf_measure(field(&reply, "server"));
    assert_eq!(bytes, server);
    let (lost, sent) = iperf_lost(rest);
    assert!(lost < sent, "{:?}", reply.body);
    assert_eq!(bytes, (sent - lost) * UDP_LEN, "{:?}", reply.body);
}

/// When the server sends, it gives no server report and cannot count loss, as iperf 2.2.2 does, so the board's own
/// `result` is the receiver's figure. The server's count may include its end datagram, which the board may not
/// count.
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_receives_udp_from_this_host_over_ethernet_without_loss() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let options = format!("-u -R -t 5 -b 10m -l {UDP_LEN}");
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Udp, &options);
    assert!(
        !reply.body.iter().any(|line| line.starts_with("server: ")),
        "{:?}",
        reply.body
    );
    let (bytes, _, rest) = iperf_measure(field(&reply, "result"));
    let (lost, datagrams) = iperf_lost(rest);
    assert_eq!(lost, 0, "{:?}", reply.body);
    assert!(datagrams > 4000, "{:?}", reply.body);
    assert!(
        (bytes..=bytes + UDP_LEN).contains(&server),
        "{server} {:?}",
        reply.body
    );
}

/// Reports the rate and the loss at 95 Mbit/s as in the sending test, from the board's own `result`.
#[test]
#[ignore = "needs the board that MCU_BOARD names, cabled to a network"]
fn iperf_receives_udp_at_95_mbit_from_this_host_over_ethernet() {
    let Some(mut client) = ethernet_console() else {
        return;
    };
    let board = board_address(&mut client);
    let options = format!("-u -R -t 5 -b 95m -l {UDP_LEN}");
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Udp, &options);
    let (bytes, _, rest) = iperf_measure(field(&reply, "result"));
    let (lost, datagrams) = iperf_lost(rest);
    assert!(lost < datagrams, "{:?}", reply.body);
    assert_eq!(bytes, (datagrams - lost) * UDP_LEN, "{:?}", reply.body);
    assert!(server >= bytes, "{server} {:?}", reply.body);
}
