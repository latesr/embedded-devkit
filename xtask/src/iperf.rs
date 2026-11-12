//! `cargo xtask iperf-server`: one test of embedded-iperf2's server on this host, the host end of the iperf
//! scenario tests. It runs from `target/debug/xtask`, a path that keeps one entry in the Mac's application
//! firewall through rebuilds. A test binary takes a new path at each build, and the firewall holds back the start
//! of each new flow into it.

use embedded_iperf2::blocking::{TcpServer, UdpServer};
use embedded_iperf2::{Report, Server, ServerConfig, Transport};
use std::io::Write;
use std::net::{TcpListener, UdpSocket};
use std::time::Duration;

/// The longest the server waits for a test to start, and the longest a test may last.
const LIMIT: Duration = Duration::from_secs(60);

/// Runs one test on `port`, 0 for a free one. It writes `listening: <port>` before it waits, and the final report
/// as [`result_line`] after the test. Fails when no test starts within [`LIMIT`], or when the test ends without a
/// final report.
pub fn run(transport: Transport, port: u16) -> Result<(), String> {
    let (bound, port) = Bound::new(transport, port)?;
    let mut out = std::io::stdout();
    // A pipe buffers stdout, and the test that started this process waits for the line before it starts the
    // board.
    writeln!(out, "listening: {port}")
        .and_then(|()| out.flush())
        .map_err(|error| error.to_string())?;
    let report = bound.serve(LIMIT)?;
    writeln!(out, "{}", result_line(&report)).map_err(|error| error.to_string())
}

/// A server bound to its port, before it waits for a test.
enum Bound {
    Tcp(TcpServer),
    Udp(UdpServer),
}

impl Bound {
    /// Binds `port` on every address, 0 for a free one, and returns the server and the port it holds.
    fn new(transport: Transport, port: u16) -> Result<(Self, u16), String> {
        let refused = |error: std::io::Error| format!("port {port}: {error}");
        match transport {
            Transport::Tcp => {
                let listener = TcpListener::bind(("0.0.0.0", port)).map_err(refused)?;
                let port = listener.local_addr().map_err(refused)?.port();
                Ok((Self::Tcp(TcpServer::new(listener)), port))
            }
            Transport::Udp => {
                let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(refused)?;
                let port = socket.local_addr().map_err(refused)?.port();
                Ok((Self::Udp(UdpServer::new(socket)), port))
            }
        }
    }

    /// Runs one test, and returns its final report. A test that has not started within `limit`, or that lasts
    /// longer, fails.
    fn serve(self, limit: Duration) -> Result<Report, String> {
        let transport = match self {
            Self::Tcp(_) => Transport::Tcp,
            Self::Udp(_) => Transport::Udp,
        };
        let config = ServerConfig {
            transport,
            duration_us: Some(u64::try_from(limit.as_micros()).unwrap_or(u64::MAX)),
            interval_us: None,
            ipv6: false,
        };
        // A reverse TCP test sends bursts of up to 128 KiB, which a smaller buffer would split into more writes.
        let mut buffer = vec![0; 128 * 1024];
        let mut server = Server::new(config, &mut buffer).map_err(|error| format!("{error:?}"))?;
        let mut last = None;
        let keep = |report: &Report| {
            if report.final_report {
                last = Some(*report);
            }
        };
        let ended = match self {
            Self::Tcp(mut driver) => driver
                .set_start_timeout(Some(limit))
                .and_then(|()| driver.run_one(&mut server, keep)),
            Self::Udp(mut driver) => driver
                .set_start_timeout(Some(limit))
                .and_then(|()| driver.run_one(&mut server, keep)),
        };
        match ended {
            Err(error) => Err(error.to_string()),
            Ok(Err(error)) => Err(format!("the test ended without a final report: {error:?}")),
            Ok(Ok(())) => last.ok_or_else(|| "the test ended without a final report".to_owned()),
        }
    }
}

/// `result: <start_us>-<end_us> us <bytes> bytes`, and for a UDP receiver ` lost <n>/<n> ooo <n>`.
fn result_line(report: &Report) -> String {
    let mut line = format!(
        "result: {}-{} us {} bytes",
        report.start_us, report.end_us, report.bytes
    );
    if let Some(udp) = report.udp {
        line.push_str(&format!(
            " lost {}/{} ooo {}",
            udp.lost, udp.datagrams, udp.out_of_order
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_iperf2::blocking::run_client;
    use embedded_iperf2::{Client, Config, Length};
    use std::net::SocketAddr;

    /// Runs one client test against a server bound on loopback, and returns the server's final report and the
    /// client's.
    fn test(config: Config) -> (Result<Report, String>, Report) {
        let (bound, port) = Bound::new(config.transport, 0).unwrap();
        let server = std::thread::spawn(move || bound.serve(LIMIT));
        let mut buffer = vec![0; 128 * 1024];
        let mut client = Client::new(config, &mut buffer).unwrap();
        let mut last = None;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        run_client(&mut client, address, |report| {
            if report.final_report {
                last = Some(*report);
            }
        })
        .unwrap()
        .unwrap();
        (server.join().unwrap(), last.unwrap())
    }

    fn config(transport: Transport, reverse: bool) -> Config {
        let base = match transport {
            Transport::Tcp => Config::tcp(),
            Transport::Udp => Config::udp(),
        };
        Config {
            reverse,
            length: Length::Bytes(147_000),
            ..base
        }
    }

    #[test]
    fn a_tcp_test_either_way_gives_the_bytes_that_the_client_counted() {
        for reverse in [false, true] {
            let (server, client) = test(config(Transport::Tcp, reverse));
            let server = server.unwrap();
            assert_eq!(server.bytes, client.bytes, "reverse {reverse}");
            assert!(server.udp.is_none());
        }
    }

    #[test]
    fn a_forward_udp_test_gives_the_receiver_s_counts() {
        let (server, _) = test(config(Transport::Udp, false));
        let server = server.unwrap();
        let udp = server.udp.unwrap();
        assert_eq!((udp.datagrams, udp.lost), (101, 0));
        assert_eq!(server.bytes, 101 * 1470);
    }

    #[test]
    fn a_test_that_never_starts_ends_at_the_limit_with_an_error() {
        let (bound, _) = Bound::new(Transport::Tcp, 0).unwrap();
        let error = bound.serve(Duration::from_millis(50)).unwrap_err();
        assert!(error.contains("no test started"), "{error}");
    }

    #[test]
    fn the_result_line_has_the_times_the_bytes_and_a_receiver_s_counts() {
        let tcp = Report {
            start_us: 0,
            end_us: 5_005_688,
            bytes: 59_506_688,
            udp: None,
            final_report: true,
            server: None,
        };
        assert_eq!(result_line(&tcp), "result: 0-5005688 us 59506688 bytes");
        let udp = Report {
            udp: Some(embedded_iperf2::UdpCounts {
                datagrams: 40392,
                lost: 85,
                out_of_order: 1,
                ..Default::default()
            }),
            ..tcp
        };
        assert_eq!(
            result_line(&udp),
            "result: 0-5005688 us 59506688 bytes lost 85/40392 ooo 1"
        );
    }
}
