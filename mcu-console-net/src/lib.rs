//! Network commands for `mcu-console`, over an embassy-net `Stack`.
//!
//! [`NetCommands`] is a command set. The application builds it with its stack and, on Wi-Fi, a type that implements
//! [`Wifi`], and composes it with its other sets. Every command starts with `net`.
//!
//! # Commands
//!
//! - `net config` writes `medium`, `mac`, `link`, `address`, `gateway`, `dns` and `mtu`. On Wi-Fi it then writes
//!   `ssid`, `bssid`, `channel` and `rssi`. A value the device does not have is `none`.
//! - `net dns <name>` writes one `address` line for each A record.
//! - `net ping <host> [count]` sends echo requests, the [`Probes`] count by default, and writes
//!   `ping: sent <n> received <n> min <t> avg <t> max <t>`, in milliseconds. Ctrl-C stops it after the probe in
//!   flight, and it writes what it sent. It fails with `no reply` when none returns.
//! - `net latency <host[:port]>` times TCP connects, port 443 by default, and aborts each with a RST once it
//!   opens. It writes `address` and
//!   `connect: sent <n> opened <n> refused <n> timeout <n> min <t> median <t> p90 <t> max <t>`. It fails with
//!   `no connection opened` when none opened.
//! - With the `iperf` feature, off by default, `net iperf <host[:port]> [-u] [-R] [-t s|-n bytes]
//!   [-l len] [-b rate] [-i s]` runs one iperf 2 client test against a server, port 5001 by default. It is a
//!   bring-up and test command, and a production build leaves it out. It writes `interval: <a>-<b> s <bytes> bytes <rate> Mbit/s` for
//!   each interval and `result:` at the end. A UDP receiver adds `lost <n>/<n> ooo <n> jitter <t> ms`, and a
//!   forward UDP test adds the server's report as `server:`. An interval is dropped when 16 already wait for
//!   the console. The application gives the buffers with `NetCommands::with_iperf`.
//! - `net wifi scan` writes `<n>: <ssid>, <bssid>, channel <c>, <rssi> dBm` for each access point, strongest
//!   first. `net wifi use <n>` takes the SSID and the BSSID from row `n`.
//! - `net wifi ssid <ssid>`, `net wifi bssid <aa:bb:cc:dd:ee:ff|any>` and `net wifi password <password>` set
//!   the network. The password is a secret argument.
//! - `net wifi join` joins it and waits for an address.
//! - `net wifi drop` drops the link and keeps the network, so the station joins it again, for testing the
//!   rejoin.
//!
//! [`Probes`] sets how many probes `net ping` and `net latency` send, how far apart and how long each waits.
//!
//! [`dhcp_config`] is the stack configuration a board crate passes to `embassy_net::new`, with a DHCP client that
//! asks again after 2 s.

#![cfg_attr(not(test), no_std)]
// The commands run on one executor beside the stack, and a stack handle is not `Send`.
#![allow(async_fn_in_trait)]

mod commands;
#[cfg(feature = "iperf")]
mod iperf;
mod probe;
mod wifi;

pub use commands::{NetCommands, Probes};
pub use probe::dhcp_config;
pub use wifi::{AccessPoint, Network, Ssid, Wifi, WifiError};
