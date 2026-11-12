//! The `net wifi` commands over a fake radio, and `net config`'s Wi-Fi lines (N-2, N-6 to N-13).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::*;
use mcu_console::{CommandResult, Commands};
use mcu_console_net::{NetCommands, Wifi, WifiError};

fn radio_with(in_range: Vec<mcu_console_net::AccessPoint>) -> FakeWifi {
    let wifi = FakeWifi::default();
    wifi.0.borrow_mut().in_range = in_range;
    wifi
}

#[test]
fn scan_numbers_the_access_points_strongest_first() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![
            access_point("home", 1, 6, -67),
            access_point("", 3, 11, -80),
            access_point("home", 2, 36, -41),
        ]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi);
        let (body, result) = send(&mut commands, "net wifi scan").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            body,
            [
                "1: home, aa:bb:cc:00:00:02, channel 36, -41 dBm",
                "2: home, aa:bb:cc:00:00:01, channel 6, -67 dBm",
                "3: (hidden), aa:bb:cc:00:00:03, channel 11, -80 dBm",
            ]
        );
    });
}

#[test]
fn access_points_of_equal_strength_keep_the_scan_s_order() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![
            access_point("a", 1, 1, -50),
            access_point("b", 2, 1, -50),
        ]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi);
        let (body, _) = send(&mut commands, "net wifi scan").await;
        assert_eq!(
            body,
            [
                "1: a, aa:bb:cc:00:00:01, channel 1, -50 dBm",
                "2: b, aa:bb:cc:00:00:02, channel 1, -50 dBm",
            ]
        );
    });
}

#[test]
fn join_does_not_warn_when_one_access_point_broadcasts_the_ssid() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![
            access_point("home", 1, 6, -67),
            access_point("other", 2, 1, -40),
        ]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi);
        send(&mut commands, "net wifi scan").await;
        send(&mut commands, "net wifi ssid home").await;
        let (body, _) = send(&mut commands, "net wifi join").await;
        assert_eq!(body, ["ssid: home", "bssid: any", "address: 10.0.0.2/24"]);
    });
}

#[test]
fn drop_asks_the_station_to_drop_the_link_and_reports_its_failure() {
    run(device_config(), async |net| {
        let wifi = FakeWifi::default();
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        assert_eq!(
            send(&mut commands, "net wifi drop").await,
            (vec![], CommandResult::Ok)
        );
        assert_eq!(wifi.0.borrow().drops, 1);
        wifi.0.borrow_mut().drop_error = Some(WifiError::Radio);
        assert_eq!(
            send(&mut commands, "net wifi drop").await.1,
            CommandResult::Err("the radio failed")
        );
        assert_eq!(wifi.0.borrow().drops, 2);
    });
}

#[test]
fn the_empty_station_has_no_radio() {
    embassy_futures::block_on(async {
        let mut none = ();
        assert_eq!(none.scan(&mut |_| {}).await, Err(WifiError::Radio));
        let ssid = mcu_console_net::Ssid::new(b"home").unwrap();
        let network = mcu_console_net::Network {
            ssid: &ssid,
            bssid: None,
            password: "",
        };
        assert_eq!(none.join(&network).await, Err(WifiError::Radio));
        assert_eq!(none.drop_link().await, Err(WifiError::Radio));
        assert_eq!(none.status(), None);
    });
}

#[test]
fn scan_keeps_the_strongest_16() {
    run(device_config(), async |net| {
        let in_range = (0..20)
            .map(|i| access_point("net", i, 1, -90 + i as i8))
            .collect();
        let mut commands = NetCommands::wifi(net.device, 1514, radio_with(in_range));
        let (body, _) = send(&mut commands, "net wifi scan").await;
        assert_eq!(body.len(), 16);
        assert_eq!(body[0], "1: net, aa:bb:cc:00:00:13, channel 1, -71 dBm");
        assert_eq!(body[15], "16: net, aa:bb:cc:00:00:04, channel 1, -86 dBm");
    });
}

#[test]
fn an_ssid_that_is_not_utf8_is_written_with_escapes() {
    run(device_config(), async |net| {
        let mut odd = access_point("x", 1, 1, -50);
        odd.ssid = mcu_console_net::Ssid::new(b"caf\xe9").unwrap();
        let mut commands = NetCommands::wifi(net.device, 1514, radio_with(vec![odd]));
        let (body, _) = send(&mut commands, "net wifi scan").await;
        assert_eq!(body, ["1: caf\\xe9, aa:bb:cc:00:00:01, channel 1, -50 dBm"]);
    });
}

#[test]
fn use_takes_the_ssid_and_bssid_of_a_row_and_join_passes_them_on() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![
            access_point("home", 1, 6, -67),
            access_point("home", 2, 36, -41),
        ]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        send(&mut commands, "net wifi scan").await;
        assert_eq!(
            send(&mut commands, "net wifi use 2").await,
            (
                vec![
                    "ssid: home".to_owned(),
                    "bssid: aa:bb:cc:00:00:01".to_owned()
                ],
                CommandResult::Ok
            )
        );
        send(&mut commands, "net wifi password secret words").await;
        let (body, result) = send(&mut commands, "net wifi join").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            body,
            [
                "ssid: home",
                "bssid: aa:bb:cc:00:00:01",
                "address: 10.0.0.2/24"
            ]
        );
        assert_eq!(
            wifi.0.borrow().joins,
            [(
                b"home".to_vec(),
                Some([0xaa, 0xbb, 0xcc, 0, 0, 1]),
                "secret words".to_owned()
            )]
        );
    });
}

#[test]
fn use_fails_for_a_row_the_last_scan_did_not_have() {
    run(device_config(), async |net| {
        let mut commands = NetCommands::wifi(
            net.device,
            1514,
            radio_with(vec![access_point("a", 1, 1, -50)]),
        );
        send(&mut commands, "net wifi scan").await;
        assert_eq!(
            send(&mut commands, "net wifi use 2").await.1,
            CommandResult::Err("no such row")
        );
        assert_eq!(
            send(&mut commands, "net wifi use 0").await.1,
            CommandResult::Err("no such row")
        );
        assert_eq!(
            send(&mut commands, "net wifi use x").await.1,
            CommandResult::Err("usage: net wifi use <n>")
        );
    });
}

#[test]
fn ssid_takes_the_rest_of_the_line_and_clears_the_bssid() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![access_point("home", 1, 6, -67)]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        send(&mut commands, "net wifi scan").await;
        send(&mut commands, "net wifi use 1").await;
        assert_eq!(
            send(&mut commands, "net wifi ssid my home  net").await,
            (vec!["ssid: my home  net".to_owned()], CommandResult::Ok)
        );
        send(&mut commands, "net wifi join").await;
        assert_eq!(wifi.0.borrow().joins[0].0, b"my home  net");
        assert_eq!(wifi.0.borrow().joins[0].1, None);
    });
}

#[test]
fn bssid_sets_one_access_point_and_any_clears_it() {
    run(device_config(), async |net| {
        let wifi = FakeWifi::default();
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        send(&mut commands, "net wifi ssid home").await;
        assert_eq!(
            send(&mut commands, "net wifi bssid AA:bb:cc:00:00:09").await,
            (
                vec!["bssid: aa:bb:cc:00:00:09".to_owned()],
                CommandResult::Ok
            )
        );
        send(&mut commands, "net wifi join").await;
        assert_eq!(
            send(&mut commands, "net wifi bssid any").await,
            (vec!["bssid: any".to_owned()], CommandResult::Ok)
        );
        send(&mut commands, "net wifi join").await;
        let bssids: Vec<_> = wifi.0.borrow().joins.iter().map(|join| join.1).collect();
        assert_eq!(bssids, [Some([0xaa, 0xbb, 0xcc, 0, 0, 9]), None]);
        assert_eq!(
            send(&mut commands, "net wifi bssid aa:bb").await.1,
            CommandResult::Err("usage: net wifi bssid <aa:bb:cc:dd:ee:ff|any>")
        );
    });
}

#[test]
fn the_password_is_a_secret_argument() {
    run(device_config(), async |net| {
        let commands = NetCommands::wifi(net.device, 1514, FakeWifi::default());
        let password = commands
            .help()
            .find(|help| help.usage.starts_with("net wifi password"))
            .unwrap();
        assert!(password.secret);
        assert_eq!(commands.help().filter(|help| help.secret).count(), 1);
    });
}

#[test]
fn join_warns_when_several_access_points_broadcast_the_ssid_and_none_is_chosen() {
    run(device_config(), async |net| {
        let wifi = radio_with(vec![
            access_point("home", 1, 6, -67),
            access_point("home", 2, 36, -41),
        ]);
        let mut commands = NetCommands::wifi(net.device, 1514, wifi);
        send(&mut commands, "net wifi scan").await;
        send(&mut commands, "net wifi ssid home").await;
        let (body, result) = send(&mut commands, "net wifi join").await;
        assert_eq!(result, CommandResult::Ok);
        assert_eq!(
            body,
            [
                "ssid: home",
                "bssid: any",
                "address: 10.0.0.2/24",
                "bssids: 2 for this ssid, see net wifi scan",
            ]
        );
    });
}

#[test]
fn join_fails_without_an_ssid_and_with_the_radio_s_reason() {
    run(device_config(), async |net| {
        let wifi = FakeWifi::default();
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        assert_eq!(
            send(&mut commands, "net wifi join").await.1,
            CommandResult::Err("no ssid")
        );
        send(&mut commands, "net wifi ssid home").await;
        for (error, reason) in [
            (WifiError::NotFound, "no such network"),
            (WifiError::Rejected, "the network refused the join"),
            (WifiError::Failed, "the join failed"),
            (WifiError::Radio, "the radio failed"),
        ] {
            wifi.0.borrow_mut().join_error = Some(error);
            assert_eq!(
                send(&mut commands, "net wifi join").await.1,
                CommandResult::Err(reason)
            );
        }
    });
}

#[test]
fn config_on_wifi_adds_the_access_point_joined_now() {
    run(device_config(), async |net| {
        let wifi = FakeWifi::default();
        let mut commands = NetCommands::wifi(net.device, 1514, wifi.clone());
        let (body, _) = send(&mut commands, "net config").await;
        assert_eq!(body[0], "medium: wifi");
        assert_eq!(
            body[7..],
            ["ssid: none", "bssid: none", "channel: none", "rssi: none"]
        );
        wifi.0.borrow_mut().joined = Some(access_point("home", 2, 36, -41));
        let (body, _) = send(&mut commands, "net config").await;
        assert_eq!(
            body[7..],
            [
                "ssid: home",
                "bssid: aa:bb:cc:00:00:02",
                "channel: 36",
                "rssi: -41 dBm"
            ]
        );
    });
}
