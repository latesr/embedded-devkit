//! `net iperf` against an embedded-iperf2 server on the far end's stack (N-15). It needs the `iperf` feature
//! (N-17).

#![cfg(feature = "iperf")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::*;
use embassy_futures::select::{Either, select};
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_time::Duration;
use embedded_iperf2::embassy::{TcpServer, UdpServer};
use embedded_iperf2::{Report, Server, ServerConfig, Transport};
use mcu_console::CommandResult;
use mcu_console_net::NetCommands;
use std::cell::RefCell;

const USAGE: &str =
    "usage: net iperf <host[:port]> [-u] [-R] [-t s|-n bytes] [-l len] [-b rate] [-i s]";

/// The final reports of the far end's server, one per test.
type Finals = RefCell<Vec<Report>>;

fn server_config(transport: Transport) -> ServerConfig {
    ServerConfig {
        transport,
        duration_us: None,
        interval_us: None,
        ipv6: false,
    }
}

/// Runs an iperf server on the far end's port 5001, for TCP or UDP, beside `test`, and returns what `test`
/// returns.
async fn with_server<T>(
    far: Stack<'_>,
    transport: Transport,
    finals: &Finals,
    test: impl Future<Output = T>,
) -> T {
    with_server_on(far, transport, 5001, finals, test).await
}

/// [`with_server`] on `port`.
async fn with_server_on<T>(
    far: Stack<'_>,
    transport: Transport,
    port: u16,
    finals: &Finals,
    test: impl Future<Output = T>,
) -> T {
    let serve = async {
        let mut work = [0; 2048];
        let mut keep = |report: &Report| {
            if report.final_report {
                finals.borrow_mut().push(*report);
            }
        };
        match transport {
            Transport::Tcp => {
                let (mut rx, mut tx) = ([0; 8192], [0; 8192]);
                let mut server = TcpServer::new(TcpSocket::new(far, &mut rx, &mut tx), port);
                loop {
                    let mut core = Server::new(server_config(transport), &mut work).unwrap();
                    let _ = server.run_one(&mut core, &mut keep).await;
                }
            }
            Transport::Udp => {
                let (mut rx_meta, mut tx_meta) =
                    ([PacketMetadata::EMPTY; 16], [PacketMetadata::EMPTY; 16]);
                let (mut rx, mut tx) = ([0; 16384], [0; 16384]);
                // The client's default datagram length, which every test here uses.
                let mut stash = [0; 1470];
                let mut socket = UdpSocket::new(far, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
                socket.bind(port).unwrap();
                let mut server = UdpServer::new(socket, &mut stash);
                loop {
                    let mut core = Server::new(server_config(transport), &mut work).unwrap();
                    let _ = server.run_one(&mut core, &mut keep).await;
                }
            }
        }
    };
    match select(serve, test).await {
        Either::First(()) => unreachable!("the server never returns"),
        Either::Second(result) => result,
    }
}

/// The buffers `net iperf` runs with in these tests.
struct Buffers {
    rx: [u8; 8192],
    tx: [u8; 8192],
    work: [u8; 2048],
}

impl Buffers {
    fn new() -> Self {
        Self {
            rx: [0; 8192],
            tx: [0; 8192],
            work: [0; 2048],
        }
    }
}

/// The start, end, byte count and rate of an `interval` or `result` value, and the rest of the line.
fn measure(text: &str) -> (f64, f64, u64, f64, &str) {
    let (span, rest) = text.split_once(" s ").unwrap();
    let (start, end) = span.split_once('-').unwrap();
    let (bytes, rest) = rest.split_once(" bytes ").unwrap();
    let (rate, rest) = rest.split_once(" Mbit/s").unwrap();
    (
        start.parse().unwrap(),
        end.parse().unwrap(),
        bytes.parse().unwrap(),
        rate.parse().unwrap(),
        rest,
    )
}

#[test]
fn a_tcp_test_of_n_bytes_writes_the_bytes_and_their_rate() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Tcp,
            &finals,
            send(&mut commands, "net iperf far.test -n 262144"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        assert_eq!(body.len(), 1, "{body:?}");
        let (start, end, bytes, rate, rest) = measure(value(&body[0], "result"));
        assert_eq!((start, bytes, rest), (0.0, 262144, ""));
        assert!(end > 0.2, "{body:?}");
        // The end is written to 0.01 s, so the rate computed from it is within 0.005 s of the written one.
        let expected = 262144.0 * 8.0 / end / 1e6;
        assert!((rate - expected).abs() <= expected * 0.03, "{body:?}");
        assert_eq!(finals.borrow()[0].bytes, 262144);
    });
}

#[test]
fn a_reverse_tcp_test_counts_the_bytes_the_server_sends() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server_on(
            net.far,
            Transport::Tcp,
            5003,
            &finals,
            send(&mut commands, "net iperf 10.0.0.1:5003 -R -n 32768"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        let (_, _, bytes, _, _) = measure(value(&body[0], "result"));
        assert_eq!(bytes, 32768);
    });
}

#[test]
fn a_reverse_tcp_test_of_a_duration_receives_for_that_long() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Tcp,
            &finals,
            send(&mut commands, "net iperf far.test -R -t 0.5"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        let (_, end, bytes, _, _) = measure(value(&body[0], "result"));
        assert!((0.45..0.7).contains(&end), "{body:?}");
        assert!(bytes > 0, "{body:?}");
    });
}

#[test]
fn a_timed_test_writes_each_interval_then_the_result() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Tcp,
            &finals,
            send(&mut commands, "net iperf far.test -t 1.5 -i 0.5"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        let (result, intervals) = body.split_last().unwrap();
        let mut total = 0;
        for (index, line) in intervals.iter().enumerate() {
            let (start, end, bytes, _, _) = measure(value(line, "interval"));
            // The core also reports the part of an interval after the deadline, while the close completes.
            let span = if index < 3 {
                (index as f64 * 0.5, (index + 1) as f64 * 0.5)
            } else {
                (1.5, end)
            };
            assert_eq!((start, end), span, "{body:?}");
            total += bytes;
        }
        assert!((3..=4).contains(&intervals.len()), "{body:?}");
        let (start, end, bytes, _, _) = measure(value(result, "result"));
        assert_eq!(start, 0.0);
        assert!((1.5..1.7).contains(&end), "{body:?}");
        assert!(bytes >= total, "{body:?}");
    });
}

#[test]
fn a_udp_test_writes_the_server_s_counts() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Udp,
            &finals,
            send(&mut commands, "net iperf far.test -u -n 14700 -b 10m"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        assert_eq!(body.len(), 2, "{body:?}");
        // Both ends count the end of the test as an 11th datagram, as iperf 2.2.2 does.
        let (_, _, bytes, _, rest) = measure(value(&body[0], "result"));
        assert_eq!((bytes, rest), (16170, ""));
        let (_, _, bytes, _, rest) = measure(value(&body[1], "server"));
        assert_eq!(bytes, 16170);
        assert!(rest.starts_with(" lost 0/11 ooo 0 jitter "), "{rest}");
        assert!(rest.ends_with(" ms"), "{rest}");
    });
}

#[test]
fn the_rate_takes_a_suffix_of_thousands() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Udp,
            &finals,
            send(&mut commands, "net iperf far.test -u -n 7350 -b 98k"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        // 5 datagrams at 98000 bit/s leave 0.12 s apart, so the last leaves at 0.48 s.
        let (_, end, _, _, _) = measure(value(&body[0], "result"));
        assert!((0.45..0.7).contains(&end), "{body:?}");
    });
}

#[test]
fn a_reverse_udp_test_writes_its_own_counts() {
    run(device_config(), async |net| {
        let finals = Finals::default();
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = with_server(
            net.far,
            Transport::Udp,
            &finals,
            send(&mut commands, "net iperf far.test -u -R -n 14700 -b 10m"),
        )
        .await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        assert_eq!(body.len(), 1, "{body:?}");
        let (_, _, bytes, _, rest) = measure(value(&body[0], "result"));
        assert_eq!(bytes, 16170);
        assert!(rest.starts_with(" lost 0/11 ooo 0 jitter "), "{rest}");
    });
}

#[test]
fn a_udp_test_without_a_server_report_says_so() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = send(&mut commands, "net iperf 10.0.0.1:5002 -u -n 1470").await;
        assert_eq!(result, CommandResult::Ok, "{body:?}");
        assert_eq!(body.len(), 2, "{body:?}");
        assert_eq!(body[1], "server: none");
    });
}

#[test]
fn without_buffers_the_command_fails() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (body, result) = send(&mut commands, "net iperf far.test").await;
        assert!(body.is_empty());
        assert_eq!(result, CommandResult::Err("no iperf buffers"));
    });
}

#[test]
fn a_bad_line_fails_with_the_usage() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        for line in [
            "net iperf",
            "net iperf far.test -x",
            "net iperf far.test -l",
            "net iperf far.test -t ten",
            "net iperf far.test -b 5q",
            "net iperf far.test:port",
            "net iperf far.test -t 1 -n 100",
        ] {
            assert_eq!(
                send(&mut commands, line).await.1,
                CommandResult::Err(USAGE),
                "{line}"
            );
        }
    });
}

#[test]
fn a_refused_connection_fails() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = send(&mut commands, "net iperf 10.0.0.1:5002 -n 1000").await;
        assert!(body.is_empty());
        assert_eq!(
            result,
            CommandResult::Err("the server refused the connection")
        );
    });
}

/// The driver gives a TCP connect 5 s, and then aborts it.
#[test]
fn a_connect_with_no_answer_times_out() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let line = format!("net iperf {NOBODY} -n 1000");
        let (body, result) = send_within(&mut commands, &line, Duration::from_secs(10)).await;
        assert!(body.is_empty());
        assert_eq!(result, CommandResult::Err("the connection timed out"));
    });
}

/// embassy-net reports no route for a UDP datagram only to an address it cannot send to, such as `0.0.0.0`.
#[test]
fn a_datagram_to_an_unaddressable_host_fails_with_no_route() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = send(&mut commands, "net iperf 0.0.0.0 -u -n 1470").await;
        assert!(body.is_empty(), "{body:?}");
        assert_eq!(result, CommandResult::Err("no route to the host"));
    });
}

/// embassy-net reports no route for a TCP connect only when the device has no address to send from.
#[test]
fn a_connect_without_an_address_fails_with_no_route() {
    run(embassy_net::Config::default(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (body, result) = send(&mut commands, "net iperf 10.0.0.1 -n 1000").await;
        assert!(body.is_empty(), "{body:?}");
        assert_eq!(result, CommandResult::Err("no route to the host"));
    });
}

#[test]
fn a_datagram_longer_than_the_buffer_fails() {
    run(device_config(), async |net| {
        let mut buffers = Buffers::new();
        let mut commands = NetCommands::ethernet(net.device, 1514).with_iperf(
            &mut buffers.rx,
            &mut buffers.tx,
            &mut buffers.work,
        );
        let (_, result) = send(&mut commands, "net iperf far.test -u -l 4000").await;
        assert_eq!(result, CommandResult::Err("-l is longer than the buffer"));
        let (_, result) = send(&mut commands, "net iperf far.test -u -l 40").await;
        assert_eq!(result, CommandResult::Err("-l is shorter than the header"));
    });
}
