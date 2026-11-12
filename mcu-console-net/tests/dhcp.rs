//! `dhcp_config` against a DHCP server on the far stack that misses the first DISCOVER (N-16).

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
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Ipv4Address, Ipv4Cidr, Stack};
use embassy_time::{Duration, Instant, WithTimeout};

const SERVER_PORT: u16 = 67;
const CLIENT_PORT: u16 = 68;

/// The DHCP message type of `message`, from its option 53, or `None` for a message without one.
fn message_type(message: &[u8]) -> Option<u8> {
    let mut at = 240;
    while at + 1 < message.len() {
        match message[at] {
            0 => at += 1,
            255 => return None,
            code => {
                let len = usize::from(message[at + 1]);
                if code == 53 {
                    return message.get(at + 2).copied();
                }
                at += 2 + len;
            }
        }
    }
    None
}

/// A reply of `kind` to `request`, giving the device its address through the far end.
fn reply(request: &[u8], kind: u8) -> Vec<u8> {
    let mut message = vec![0u8; 240];
    message[0] = 2;
    message[1] = 1;
    message[2] = 6;
    message[4..8].copy_from_slice(&request[4..8]);
    message[16..20].copy_from_slice(&DEVICE.octets());
    message[20..24].copy_from_slice(&FAR.octets());
    message[28..44].copy_from_slice(&request[28..44]);
    message[236..240].copy_from_slice(&[99, 130, 83, 99]);
    message.extend_from_slice(&[53, 1, kind]);
    message.extend_from_slice(&[54, 4]);
    message.extend_from_slice(&FAR.octets());
    message.extend_from_slice(&[51, 4, 0, 0, 14, 16]);
    message.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
    message.extend_from_slice(&[3, 4]);
    message.extend_from_slice(&FAR.octets());
    message.push(255);
    message
}

/// How long the far end's DHCP server takes to start. The device's first DISCOVERs come before it.
const SERVER_START: Duration = Duration::from_millis(500);

/// Answers DHCP on the far stack once [`SERVER_START`] has passed, and ignores what comes before it.
async fn serve_after_starting(stack: Stack<'_>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx = [0; 1024];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx = [0; 1024];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    socket.bind(SERVER_PORT).unwrap();
    let broadcast = IpEndpoint::new(Ipv4Address::BROADCAST.into(), CLIENT_PORT);
    let ready = Instant::now() + SERVER_START;
    let mut request = [0u8; 1024];
    loop {
        let (len, _) = socket.recv_from(&mut request).await.unwrap();
        if Instant::now() < ready {
            continue;
        }
        let request = &request[..len];
        let answer = match message_type(request) {
            Some(1) => reply(request, 2),
            Some(3) => reply(request, 5),
            _ => continue,
        };
        socket.send_to(&answer, broadcast).await.unwrap();
    }
}

/// The device asks again 2 s after the DISCOVERs the starting server missed, so the lease comes about 2 s in.
#[test]
fn a_lease_comes_2_s_after_discovers_the_server_missed() {
    run(mcu_console_net::dhcp_config(), async |net| {
        let start = Instant::now();
        let leased = select(
            serve_after_starting(net.far),
            net.device
                .wait_config_up()
                .with_timeout(Duration::from_secs(15)),
        )
        .await;
        let Either::Second(Ok(())) = leased else {
            panic!("no lease within 15 s");
        };
        let elapsed = start.elapsed();
        assert!(
            (Duration::from_secs(2)..Duration::from_millis(3_500)).contains(&elapsed),
            "the lease took {} ms",
            elapsed.as_millis()
        );
        assert_eq!(
            net.device.config_v4().unwrap().address,
            Ipv4Cidr::new(DEVICE, 24)
        );
    });
}
