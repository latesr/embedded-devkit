//! `net iperf`: one iperf 2 client test over the embedded-iperf2 driver (N-15).
//!
//! The driver hands each report to a plain function while the test runs, and the console writes asynchronously.
//! The reports wait in [`Waiting`] until a writer beside the test takes them.

use crate::commands::Millis;
use crate::probe;
use core::cell::{Cell, RefCell};
use core::fmt::{self, Display, Formatter};
use embassy_futures::join::join;
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Timer};
use embedded_iperf2::wire::ServerReport;
use embedded_iperf2::{Client, Config, ConfigError, Length, Report, TestError, UdpCounts};
use mcu_console::{Args, CommandResult, Reply};

pub(crate) const USAGE: &str =
    "usage: net iperf <host[:port]> [-u] [-R] [-t s|-n bytes] [-l len] [-b rate] [-i s]";

/// The port an iperf 2.2.2 server listens on.
const PORT: u16 = 5001;

/// How often the writer looks for a waiting report.
const WRITE_POLL: Duration = Duration::from_millis(50);

/// How many interval reports wait for the console. One more is dropped.
const WAITING: usize = 16;

/// The buffers of `net iperf`'s socket and client.
pub(crate) struct Buffers<'d> {
    pub(crate) rx: &'d mut [u8],
    pub(crate) tx: &'d mut [u8],
    pub(crate) work: &'d mut [u8],
}

/// Runs `net iperf` with the rest of its line in `args`.
pub(crate) async fn run<R: Reply>(
    stack: Stack<'_>,
    buffers: Option<&mut Buffers<'_>>,
    args: Args<'_>,
    reply: &mut R,
) -> CommandResult {
    let Some((host, config)) = parse(args) else {
        return CommandResult::Err(USAGE);
    };
    let Some(buffers) = buffers else {
        return CommandResult::Err("no iperf buffers");
    };
    let mut client = match Client::new(config, buffers.work) {
        Ok(client) => client,
        Err(ConfigError::BufLenTooLong) => {
            return CommandResult::Err("-l is longer than the buffer");
        }
        Err(ConfigError::BufLenTooShort) => {
            return CommandResult::Err("-l is shorter than the header");
        }
    };
    let address = match probe::resolve(stack, host).await {
        Ok(address) => address,
        Err(reason) => return CommandResult::Err(reason),
    };
    let waiting = RefCell::new(Waiting::default());
    let last = Cell::new(None);
    let over = Cell::new(false);
    let test = async {
        let outcome = probe::iperf(
            stack,
            buffers.rx,
            buffers.tx,
            &mut client,
            IpEndpoint::new(address.into(), config.port),
            |report| {
                if report.final_report {
                    last.set(Some(*report));
                } else {
                    waiting.borrow_mut().push(*report);
                }
            },
        )
        .await;
        over.set(true);
        outcome
    };
    let write = async {
        loop {
            let finished = over.get();
            loop {
                // The borrow ends before the write, so the test can push a report while the console writes.
                let next = waiting.borrow_mut().pop();
                let Some(report) = next else { break };
                reply.field("interval", Measured::of(&report)).await;
            }
            if finished {
                break;
            }
            Timer::after(WRITE_POLL).await;
        }
    };
    let (outcome, ()) = join(test, write).await;
    if let Some(report) = last.get() {
        reply.field("result", Measured::of(&report)).await;
        if config.transport == embedded_iperf2::Transport::Udp && !config.reverse {
            match report.server {
                Some(server) => reply.field("server", Measured::from_server(&server)).await,
                None => reply.field("server", "none").await,
            }
        }
    }
    match outcome {
        Ok(Ok(())) => CommandResult::Ok,
        Ok(Err(TestError::Refused)) => CommandResult::Err("the server refused the test"),
        Ok(Err(TestError::NoData)) => CommandResult::Err("no data arrived"),
        Ok(Err(TestError::Protocol)) => CommandResult::Err("the server broke the protocol"),
        Err(reason) => CommandResult::Err(reason),
    }
}

/// The host and the test of a `net iperf` line, or `None` when the line does not parse.
fn parse<'a>(mut args: Args<'a>) -> Option<(&'a str, Config)> {
    let target = args.next()?;
    let (host, port) = match target.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (target, PORT),
    };
    let (mut udp, mut reverse) = (false, false);
    let (mut length, mut buf_len, mut rate, mut interval_us) = (None, None, None, None);
    while let Some(option) = args.next() {
        match option {
            "-u" => udp = true,
            "-R" => reverse = true,
            "-t" | "-n" if length.is_some() => return None,
            "-t" => {
                length = Some(Length::Time {
                    us: seconds(args.next()?)?,
                })
            }
            "-n" => length = Some(Length::Bytes(args.next()?.parse().ok()?)),
            "-l" => buf_len = Some(args.next()?.parse().ok()?),
            "-b" => rate = Some(bits_per_second(args.next()?)?),
            "-i" => interval_us = Some(seconds(args.next()?)?),
            _ => return None,
        }
    }
    let defaults = if udp { Config::udp() } else { Config::tcp() };
    Some((
        host,
        Config {
            reverse,
            length: length.unwrap_or(defaults.length),
            buf_len: buf_len.unwrap_or(defaults.buf_len),
            rate: rate.unwrap_or(defaults.rate),
            interval_us,
            port,
            ..defaults
        },
    ))
}

/// `1`, `1.5` or `0.25` seconds, in microseconds.
fn seconds(text: &str) -> Option<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 6 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut micros: u64 = whole.parse::<u64>().ok()?.checked_mul(1_000_000)?;
    let mut scale = 100_000u64;
    for digit in fraction.bytes() {
        micros = micros.checked_add(u64::from(digit.wrapping_sub(b'0')).checked_mul(scale)?)?;
        scale /= 10;
    }
    Some(micros)
}

/// A rate such as `800`, `98k`, `10m` or `1g`, in bits per second. The suffixes multiply by 1000, 1000000 and
/// 1000000000.
fn bits_per_second(text: &str) -> Option<u64> {
    let (digits, scale) = match text.as_bytes().last()? {
        b'k' => (text.get(..text.len().wrapping_sub(1))?, 1_000),
        b'm' => (text.get(..text.len().wrapping_sub(1))?, 1_000_000),
        b'g' => (text.get(..text.len().wrapping_sub(1))?, 1_000_000_000),
        _ => (text, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

/// The interval reports that wait for the console, oldest first.
#[derive(Default)]
struct Waiting {
    reports: [Option<Report>; WAITING],
    next_in: usize,
    next_out: usize,
}

impl Waiting {
    /// Keeps `report`, or drops it when [`WAITING`] reports already wait.
    fn push(&mut self, report: Report) {
        if let Some(slot @ None) = self.reports.get_mut(self.next_in) {
            *slot = Some(report);
            self.next_in = self.next_in.wrapping_add(1).wrapping_rem(WAITING);
        }
    }

    fn pop(&mut self) -> Option<Report> {
        let report = self.reports.get_mut(self.next_out)?.take()?;
        self.next_out = self.next_out.wrapping_add(1).wrapping_rem(WAITING);
        Some(report)
    }
}

/// `<a>-<b> s <bytes> bytes <rate> Mbit/s`, and a receiver's counts.
struct Measured {
    start_us: u64,
    end_us: u64,
    bytes: u64,
    udp: Option<UdpCounts>,
}

impl Measured {
    fn of(report: &Report) -> Self {
        Self {
            start_us: report.start_us,
            end_us: report.end_us,
            bytes: report.bytes,
            udp: report.udp,
        }
    }

    fn from_server(server: &ServerReport) -> Self {
        let micros = |sec: u32, usec: u32| {
            u64::from(sec)
                .saturating_mul(1_000_000)
                .saturating_add(u64::from(usec))
        };
        Self {
            start_us: 0,
            end_us: micros(server.duration.sec, server.duration.usec),
            bytes: server.bytes,
            udp: Some(UdpCounts {
                datagrams: server.datagrams,
                lost: server.lost,
                out_of_order: server.out_of_order,
                jitter_ns: micros(server.jitter.sec, server.jitter.usec).saturating_mul(1_000),
            }),
        }
    }
}

impl Display for Measured {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let elapsed = self.end_us.saturating_sub(self.start_us);
        // Bytes per microsecond times 8 is Mbit/s, and 800 gives hundredths of it.
        let hundredths = self
            .bytes
            .saturating_mul(800)
            .checked_div(elapsed)
            .unwrap_or(0);
        write!(
            f,
            "{}-{} s {} bytes {}.{:02} Mbit/s",
            Seconds(self.start_us),
            Seconds(self.end_us),
            self.bytes,
            hundredths / 100,
            hundredths % 100
        )?;
        if let Some(udp) = self.udp {
            write!(
                f,
                " lost {}/{} ooo {} jitter {} ms",
                udp.lost,
                udp.datagrams,
                udp.out_of_order,
                Millis(udp.jitter_ns / 1_000)
            )?;
        }
        Ok(())
    }
}

/// Microseconds written as seconds with two decimals, such as `1.50`.
struct Seconds(u64);

impl Display for Seconds {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{:02}",
            self.0 / 1_000_000,
            self.0 % 1_000_000 / 10_000
        )
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;

    fn report(start_us: u64) -> Report {
        Report {
            start_us,
            end_us: start_us + 1,
            bytes: 0,
            udp: None,
            final_report: false,
            server: None,
        }
    }

    #[test]
    fn seconds_take_up_to_six_decimals() {
        assert_eq!(seconds("2"), Some(2_000_000));
        assert_eq!(seconds("1.5"), Some(1_500_000));
        assert_eq!(seconds("0.000001"), Some(1));
        assert_eq!(seconds("0.0000001"), None);
        assert_eq!(seconds("1.x"), None);
        assert_eq!(seconds("ten"), None);
    }

    #[test]
    fn a_rate_takes_a_suffix_of_thousands_millions_or_billions() {
        assert_eq!(bits_per_second("800"), Some(800));
        assert_eq!(bits_per_second("98k"), Some(98_000));
        assert_eq!(bits_per_second("10m"), Some(10_000_000));
        assert_eq!(bits_per_second("2g"), Some(2_000_000_000));
        assert_eq!(bits_per_second("k"), None);
        assert_eq!(bits_per_second("5q"), None);
    }

    #[test]
    fn waiting_reports_come_out_in_order_and_one_past_the_limit_is_dropped() {
        let mut waiting = Waiting::default();
        for start in 0..=WAITING as u64 {
            waiting.push(report(start));
        }
        for start in 0..WAITING as u64 {
            assert_eq!(waiting.pop().map(|report| report.start_us), Some(start));
        }
        assert_eq!(waiting.pop(), None);
        waiting.push(report(99));
        assert_eq!(waiting.pop().map(|report| report.start_us), Some(99));
    }

    #[test]
    fn a_measure_writes_seconds_bytes_and_megabits() {
        let measured = Measured {
            start_us: 1_000_000,
            end_us: 1_500_000,
            bytes: 625_000,
            udp: None,
        };
        assert_eq!(
            measured.to_string(),
            "1.00-1.50 s 625000 bytes 10.00 Mbit/s"
        );
        let empty = Measured {
            end_us: 1_000_000,
            ..measured
        };
        assert_eq!(empty.to_string(), "1.00-1.00 s 625000 bytes 0.00 Mbit/s");
    }
}
