//! `net config`, `net dns`, `net ping` and `net latency` over two real embassy-net stacks (N-1 to N-5).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::*;
use embassy_net::Config;
use embassy_time::Duration;
use mcu_console::{CommandResult, Commands};
use mcu_console_net::NetCommands;

#[test]
fn config_reports_the_address_the_gateway_the_resolver_and_the_given_medium_and_mtu() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (body, result) = send(&mut commands, "net config").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            body,
            [
                "medium: ethernet",
                "mac: 02:00:00:00:00:02",
                "link: up",
                "address: 10.0.0.2/24",
                "gateway: 10.0.0.1",
                "dns: 10.0.0.1",
                "mtu: 1514",
            ]
        );
    });
}

#[test]
fn config_lists_every_resolver() {
    let mut config = device_config();
    if let Config {
        ipv4: embassy_net::ConfigV4::Static(ref mut v4),
        ..
    } = config
    {
        v4.dns_servers
            .push(embassy_net::Ipv4Address::new(10, 0, 0, 3))
            .unwrap();
    }
    run(config, async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (body, _) = send(&mut commands, "net config").await;
        assert_eq!(body[5], "dns: 10.0.0.1, 10.0.0.3");
    });
}

#[test]
fn config_without_an_address_writes_none() {
    run(Config::default(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (body, _) = send(&mut commands, "net config").await;
        assert_eq!(body[3..6], ["address: none", "gateway: none", "dns: none"]);
    });
}

#[test]
fn dns_writes_every_a_record_the_resolver_returns() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (body, result) = send(&mut commands, "net dns two.test").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(body, ["address: 10.0.0.1", "address: 10.0.0.3"]);
    });
}

#[test]
fn dns_writes_an_address_back_without_a_query() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        assert_eq!(
            send(&mut commands, "net dns 10.0.0.7").await,
            (vec!["address: 10.0.0.7".to_owned()], CommandResult::Ok)
        );
    });
}

#[test]
fn dns_fails_for_a_name_with_no_record() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let (_, result) = send(&mut commands, "net dns none.test").await;
        assert_eq!(result, CommandResult::Err("lookup failed"));
    });
}

#[test]
fn ping_counts_each_echo_and_times_the_round_trip_in_milliseconds() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net ping far.test").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(body.len(), 1);
        let summary = value(&body[0], "ping");
        assert!(summary.starts_with("sent 3 received 3 min "), "{summary}");
        let (min, avg, max) = (
            millis(summary, "min"),
            millis(summary, "avg"),
            millis(summary, "max"),
        );
        let round_trip = 2.0 * DELAY.as_micros() as f64 / 1000.0;
        assert!(min >= round_trip, "{summary}");
        assert!(min <= avg && avg <= max, "{summary}");
        assert!(max < f64::from(QUICK.timeout_ms), "{summary}");
    });
}

#[test]
fn ping_sends_the_count_the_line_gives_beyond_the_samples_latency_keeps() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net ping far.test 70").await;
        assert_eq!(result, CommandResult::Ok);
        let summary = value(&body[0], "ping");
        assert!(summary.starts_with("sent 70 received 70 min "), "{summary}");
        let (min, avg, max) = (
            millis(summary, "min"),
            millis(summary, "avg"),
            millis(summary, "max"),
        );
        assert!(min <= avg && avg <= max, "{summary}");
    });
}

#[test]
fn ping_asked_to_stop_writes_the_probes_it_sent_and_succeeds() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send_and_stop(
            &mut commands,
            "net ping far.test 1000",
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(result, CommandResult::Ok);
        let summary = value(&body[0], "ping");
        let counts: Vec<u32> = summary
            .split_whitespace()
            .skip(1)
            .step_by(2)
            .take(2)
            .map(|count| count.parse().unwrap())
            .collect();
        assert_eq!(counts[0], counts[1], "{summary}");
        assert!(counts[0] < 1000, "{summary}");
    });
}

#[test]
fn ping_refuses_a_count_that_is_not_a_positive_number() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let usage = CommandResult::Err("usage: net ping <host> [count]");
        assert_eq!(
            send(&mut commands, "net ping far.test x").await,
            (vec![], usage)
        );
        assert_eq!(
            send(&mut commands, "net ping far.test 0").await,
            (vec![], usage)
        );
    });
}

#[test]
fn ping_with_no_reply_counts_the_losses_and_fails() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net ping 10.0.0.9").await;
        assert_eq!(body, ["ping: sent 3 received 0"]);
        assert_eq!(result, CommandResult::Err("no reply"));
    });
}

#[test]
fn latency_times_each_connect_to_port_443_by_default() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net latency far.test").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(body[0], "address: 10.0.0.1:443");
        let connect = value(&body[1], "connect");
        assert!(
            connect.starts_with("sent 3 opened 3 refused 0 timeout 0 min "),
            "{connect}"
        );
        let round_trip = 2.0 * DELAY.as_micros() as f64 / 1000.0;
        let (min, median, p90, max) = (
            millis(connect, "min"),
            millis(connect, "median"),
            millis(connect, "p90"),
            millis(connect, "max"),
        );
        assert!(min >= round_trip, "{connect}");
        assert!(min <= median && median <= p90 && p90 <= max, "{connect}");
    });
}

#[test]
fn latency_aborts_each_connection_after_the_handshake() {
    run(device_config(), async |net| {
        let one = mcu_console_net::Probes { count: 1, ..QUICK };
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(one, one);
        send(&mut commands, "net latency 10.0.0.1").await;
        const SYN: u8 = 0x02;
        const ACK: u8 = 0x10;
        const RST: u8 = 0x04;
        let flags: Vec<(usize, u8)> = net
            .tcp_flags(OPEN_PORT)
            .into_iter()
            .map(|(side, flags)| (side, flags & (SYN | ACK | RST)))
            .collect();
        assert_eq!(
            flags[..4],
            [(0, SYN), (1, SYN | ACK), (0, ACK), (0, RST | ACK)]
        );
    });
}

#[test]
fn latency_counts_a_refusal_and_fails_when_nothing_opened() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net latency 10.0.0.1:444").await;
        assert_eq!(
            body,
            [
                "address: 10.0.0.1:444",
                "connect: sent 3 opened 0 refused 3 timeout 0"
            ]
        );
        assert_eq!(result, CommandResult::Err("no connection opened"));
    });
}

#[test]
fn latency_counts_a_connect_with_no_answer_as_a_timeout() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514).with_probes(QUICK, QUICK);
        let (body, result) = send(&mut commands, "net latency 10.0.0.9").await;
        assert_eq!(body[1], "connect: sent 3 opened 0 refused 0 timeout 3");
        assert_eq!(result, CommandResult::Err("no connection opened"));
    });
}

#[test]
fn a_missing_or_unknown_subcommand_fails_with_the_usage() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let usage = CommandResult::Err(if cfg!(feature = "iperf") {
            "usage: net config|dns|ping|latency|iperf|wifi"
        } else {
            "usage: net config|dns|ping|latency|wifi"
        });
        assert_eq!(send(&mut commands, "net").await.1, usage);
        assert_eq!(send(&mut commands, "net frob").await.1, usage);
        if !cfg!(feature = "iperf") {
            assert_eq!(send(&mut commands, "net iperf 10.0.0.1").await.1, usage);
        }
        assert_eq!(
            send(&mut commands, "net ping").await.1,
            CommandResult::Err("usage: net ping <host> [count]")
        );
        assert_eq!(send(&mut commands, "other").await.1, CommandResult::Unknown);
    });
}

#[test]
fn ethernet_has_no_wifi_commands() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::ethernet(net.device, 1514);
        let usages: Vec<&str> = commands.help().map(|help| help.usage).collect();
        let mut expected = vec![
            "net config",
            "net dns <name>",
            "net ping <host> [count]",
            "net latency <host[:port]>",
        ];
        if cfg!(feature = "iperf") {
            expected.push(
                "net iperf <host[:port]> [-u] [-R] [-t s|-n bytes] [-l len] [-b rate] [-i s]",
            );
        }
        assert_eq!(usages, expected);
        assert_eq!(
            send(&mut commands, "net wifi scan").await.1,
            CommandResult::Err("no wifi on this device")
        );
    });
}
