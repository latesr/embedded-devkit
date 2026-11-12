//! A board crate's Wi-Fi, through the `console_wifi` example and `mcu-console-net`'s commands (B-7, B-15,
//! B-19, B-20, N-2 to N-15).
//!
//! The tests join the network that `MCU_WIFI_SSID` and `MCU_WIFI_PASSWORD` name. The password goes to the board
//! as a secret argument and is never printed. With either unset, or on a board without Wi-Fi, each test says so
//! and returns. All probe
//! traffic stays on the local network: pings go to the lease's gateway and from this host to the board, and
//! connects and iperf tests go to a server on this host. The iperf tests' server is embedded-iperf2's, run as
//! `cargo xtask iperf-server`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::{
    PING_PAYLOAD, TIMEOUT, Transport, board_crate, console, field, iperf_against_host, iperf_lost,
    iperf_measure, ping_from_host,
};
use mcu_run::{Client, CommandResult, Reply};
use mcu_scenario::{Console, env_or_skip};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::time::{Duration, Instant};

/// Long enough for a join and a DHCP lease, or for 20 connects 0.5 s apart.
const SLOW: Duration = Duration::from_secs(60);

/// The test network's SSID and password, or `None` for a board without Wi-Fi or when either is unset.
fn network() -> Option<(String, String)> {
    if !board_crate().wifi {
        eprintln!("skipped: {} has no Wi-Fi", board_crate().folder);
        return None;
    }
    let ssid = env_or_skip("MCU_WIFI_SSID")?;
    let password = env_or_skip("MCU_WIFI_PASSWORD")?;
    Some((ssid, password))
}

/// Sets the network and joins it, and returns the join's reply.
fn join(client: &mut Client<impl Read + Write>, ssid: &str, password: &str) -> Reply {
    set_network(client, ssid, password);
    client.send("net wifi join", SLOW).unwrap()
}

/// Sets the network to join.
fn set_network(client: &mut Client<impl Read + Write>, ssid: &str, password: &str) {
    assert_eq!(
        client
            .send(&format!("net wifi ssid {ssid}"), TIMEOUT)
            .unwrap()
            .result,
        CommandResult::Ok
    );
    assert_eq!(
        client
            .send(&format!("net wifi password {password}"), TIMEOUT)
            .unwrap()
            .result,
        CommandResult::Ok
    );
}

fn is_mac(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2
                && part
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
}

/// `-NN dBm`, a signal strength below 0 dBm.
fn is_dbm(text: &str) -> bool {
    text.strip_suffix(" dBm")
        .and_then(|number| number.parse::<i8>().ok())
        .is_some_and(|rssi| rssi < 0)
}

/// The access points around the test network broadcast hidden networks too, which a scan leaves out unless asked
/// (N-6). A board with none in range fails this test.
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn scan_lists_hidden_networks() {
    if network().is_none() {
        return;
    }
    let mut client = console("console_wifi");
    let reply = client.send_within("net wifi scan", SLOW).unwrap();
    assert_eq!(reply.result, CommandResult::Ok);
    assert!(
        reply.body.iter().any(|line| line.contains(": (hidden), ")),
        "no hidden network among {} rows",
        reply.body.len()
    );
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn scan_lists_the_test_network() {
    let Some((ssid, _)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let reply = client.send_within("net wifi scan", SLOW).unwrap();
    assert_eq!(reply.result, CommandResult::Ok);
    for (index, line) in reply.body.iter().enumerate() {
        let (number, rest) = line.split_once(": ").unwrap();
        assert_eq!(number, (index + 1).to_string());
        let parts: Vec<&str> = rest.rsplitn(3, ", ").collect();
        assert!(is_dbm(parts[0]), "{line}");
        assert!(parts[1].starts_with("channel "), "{line}");
        let (_, bssid) = parts[2].rsplit_once(", ").unwrap();
        assert!(is_mac(bssid), "{line}");
    }
    assert!(
        reply
            .body
            .iter()
            .any(|line| line.contains(&format!(": {ssid}, "))),
        "the test network is not in {:?}",
        reply.body
    );
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn join_gets_an_address_and_config_reports_the_network() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &password);
    assert_eq!(joined.result, CommandResult::Ok, "{:?}", joined.body);
    assert_eq!(field(&joined, "ssid"), ssid);
    let address = field(&joined, "address").to_owned();
    let config = client.send("net config").unwrap();
    assert_eq!(field(&config, "medium"), "wifi");
    assert_eq!(field(&config, "link"), "up");
    assert_eq!(field(&config, "address"), address);
    assert_eq!(field(&config, "mtu"), "1514");
    assert_eq!(field(&config, "ssid"), ssid);
    assert!(is_mac(field(&config, "bssid")), "{:?}", config.body);
    assert!(
        field(&config, "channel").parse::<u8>().is_ok(),
        "{:?}",
        config.body
    );
    assert!(is_dbm(field(&config, "rssi")), "{:?}", config.body);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn use_joins_the_access_point_of_the_chosen_row() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let scan = client.send_within("net wifi scan", SLOW).unwrap();
    let row = scan
        .body
        .iter()
        .find(|line| line.contains(&format!(": {ssid}, ")))
        .unwrap();
    let number = row.split_once(": ").unwrap().0;
    let chosen = client.send(&format!("net wifi use {number}")).unwrap();
    let bssid = field(&chosen, "bssid").to_owned();
    assert_eq!(
        client
            .send(&format!("net wifi password {password}"))
            .unwrap()
            .result,
        CommandResult::Ok
    );
    let joined = client.send_within("net wifi join", SLOW).unwrap();
    assert_eq!(joined.result, CommandResult::Ok, "{:?}", joined.body);
    assert_eq!(field(&joined, "bssid"), bssid);
    let config = client.send("net config").unwrap();
    assert_eq!(field(&config, "bssid"), bssid);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn a_wrong_password_is_rejected() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &format!("{password}-wrong"));
    assert_eq!(
        joined.result,
        CommandResult::Err("the network refused the join".to_owned())
    );
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn ping_reaches_the_gateway() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    assert_eq!(
        join(&mut client, &ssid, &password).result,
        CommandResult::Ok
    );
    let config = client.send("net config").unwrap();
    let gateway = field(&config, "gateway").to_owned();
    let ping = client
        .send_within(&format!("net ping {gateway}"), SLOW)
        .unwrap();
    assert_eq!(ping.result, CommandResult::Ok, "{:?}", ping.body);
    let summary = field(&ping, "ping");
    assert!(summary.starts_with("sent 5 received "), "{summary}");
    assert!(summary.contains(" min "), "{summary}");
}

/// Wi-Fi can lose a ping, so one reply is enough to show the board answers.
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn the_board_answers_pings_from_this_host() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &password);
    assert_eq!(joined.result, CommandResult::Ok);
    let board = field(&joined, "address").split('/').next().unwrap();
    assert!(ping_from_host(board, 5, PING_PAYLOAD) > 0);
}

/// The board's echo reply is as long as the request, so a 1472-byte payload needs a 1500-byte IP MTU (B-15). At
/// esp-radio's default `wifi_mtu` of 1492 the IP MTU is 1478, and smoltcp drops every reply over 1450 bytes.
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn the_board_answers_pings_of_a_full_ip_packet() {
    const FULL_PACKET: usize = 1500 - 20 - 8;
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &password);
    assert_eq!(joined.result, CommandResult::Ok);
    let board = field(&joined, "address").split('/').next().unwrap();
    assert!(ping_from_host(board, 10, FULL_PACKET) > 0);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn latency_times_connects_to_a_listener_on_this_host() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &password);
    assert_eq!(joined.result, CommandResult::Ok);
    let board: Ipv4Addr = field(&joined, "address")
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
    assert_eq!(field(&latency, "address"), format!("{host}:{port}"));
    let connect = field(&latency, "connect");
    assert!(connect.starts_with("sent 20 opened "), "{connect}");
    assert!(connect.contains(" refused 0 "), "{connect}");
}

/// After a drop the station joins the same network again by itself, and gets an address (B-19, N-14).
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn after_a_drop_the_station_joins_the_network_again() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    assert_eq!(
        join(&mut client, &ssid, &password).result,
        CommandResult::Ok
    );
    let dropped = client.send_within("net wifi drop", SLOW).unwrap();
    assert_eq!(dropped.result, CommandResult::Ok, "{:?}", dropped.body);
    let config = client.send("net config").unwrap();
    assert_eq!(field(&config, "link"), "down");
    assert_eq!(field(&config, "ssid"), "none");
    let deadline = Instant::now() + Duration::from_secs(20);
    let rejoined = loop {
        std::thread::sleep(Duration::from_millis(500));
        let config = client.send("net config").unwrap();
        if field(&config, "link") == "up" && field(&config, "address") != "none" {
            break config;
        }
        assert!(
            Instant::now() < deadline,
            "no rejoin within 20 s: {:?}",
            config.body
        );
    };
    assert_eq!(field(&rejoined, "ssid"), ssid);
}

/// Two Ctrl-Cs drop a join part way, and the next join still gets an address. The radio goes on with the
/// dropped join on its own, so this checks that it does not get in the way of the next.
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn a_join_dropped_part_way_does_not_stop_the_next_join() {
    let Some((ssid, password)) = network() else {
        return;
    };
    let mut client = console("console_wifi");
    set_network(&mut client, &ssid, &password);
    let dropped = client
        .send_interrupted("net wifi join", Duration::from_millis(300), SLOW)
        .unwrap();
    assert_eq!(
        dropped.result,
        CommandResult::Err("interrupted".to_owned()),
        "{:?}",
        dropped.body
    );
    let joined = client.send_within("net wifi join", SLOW).unwrap();
    assert_eq!(joined.result, CommandResult::Ok, "{:?}", joined.body);
    let config = client.send("net config").unwrap();
    assert_ne!(field(&config, "address"), "none");
}

/// The console of a board joined to the test network, and the board's address, or `None` without a network.
fn joined_for_iperf() -> Option<(Console, Ipv4Addr)> {
    let (ssid, password) = network()?;
    let mut client = console("console_wifi");
    let joined = join(&mut client, &ssid, &password);
    assert_eq!(joined.result, CommandResult::Ok, "{:?}", joined.body);
    let board = field(&joined, "address")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    Some((client, board))
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn iperf_sends_tcp_to_this_host_over_wifi() {
    let Some((mut client, board)) = joined_for_iperf() else {
        return;
    };
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Tcp, "-t 5");
    let (bytes, rate, _) = iperf_measure(field(&reply, "result"));
    assert_eq!(bytes, server);
    assert!(rate > 1.0, "{:?}", reply.body);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn iperf_receives_tcp_from_this_host_over_wifi() {
    let Some((mut client, board)) = joined_for_iperf() else {
        return;
    };
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Tcp, "-R -t 5");
    let (bytes, rate, _) = iperf_measure(field(&reply, "result"));
    assert_eq!(bytes, server);
    assert!(rate > 1.0, "{:?}", reply.body);
}

/// Wi-Fi loses datagrams that a cable does not, so the test allows 5 in every hundred.
#[test]
#[ignore = "needs the board that MCU_BOARD names and a Wi-Fi network"]
fn iperf_sends_udp_to_this_host_over_wifi() {
    let Some((mut client, board)) = joined_for_iperf() else {
        return;
    };
    let (reply, server) = iperf_against_host(&mut client, board, Transport::Udp, "-u -t 5 -b 10m");
    let (bytes, _, rest) = iperf_measure(field(&reply, "server"));
    assert_eq!(bytes, server);
    let (lost, sent) = iperf_lost(rest);
    assert!(sent > 4000, "{:?}", reply.body);
    assert!(lost * 20 <= sent, "{:?}", reply.body);
}
