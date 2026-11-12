//! Selecting one board from a list of fake boards by a device value (M-3).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use mcu_run::{Board, Error, select};

fn board(selector: &str, name: &str) -> Board {
    Board {
        selector: selector.to_owned(),
        name: name.to_owned(),
        chip: None,
    }
}

fn boards() -> Vec<Board> {
    vec![
        board("303a:1001:aa:bb:cc:dd:ee:01", "ESP JTAG"),
        board("303a:1001:12:34:56:78:9a:02", "ESP JTAG"),
        board("0483:3754:0123456789abcdef01234567", "STLink V3"),
    ]
}

#[test]
fn a_probe_substring_that_names_one_board_selects_it_regardless_of_case() {
    let boards = boards();
    assert_eq!(
        select(&boards, Some("probe://DD:EE:01")).unwrap(),
        &boards[0]
    );
    assert_eq!(select(&boards, Some("probe://0483:")).unwrap(), &boards[2]);
    assert_eq!(
        select(&boards, Some("probe://303a:1001:12")).unwrap(),
        &boards[1]
    );
}

#[test]
fn no_device_picks_the_only_board() {
    let one = vec![board("0483:3754:0123", "STLink V3")];
    assert_eq!(select(&one, None).unwrap(), &one[0]);
}

#[test]
fn no_device_with_several_boards_lists_them_all() {
    let boards = boards();
    let error = select(&boards, None).unwrap_err();
    assert_eq!(
        error,
        Error::ManyBoards {
            selector: None,
            boards: boards.clone(),
        }
    );
    let text = error.to_string();
    assert!(text.starts_with("3 boards are attached."), "{text}");
    for board in &boards {
        assert!(text.contains(&board.selector), "{text}");
    }
}

#[test]
fn a_probe_that_matches_nothing_lists_every_board() {
    let boards = boards();
    let error = select(&boards, Some("probe://dead:beef")).unwrap_err();
    assert_eq!(
        error,
        Error::NoBoard {
            selector: Some("probe://dead:beef".to_owned()),
            boards: boards.clone(),
        }
    );
    let text = error.to_string();
    assert!(
        text.starts_with("no attached board matches probe://dead:beef."),
        "{text}"
    );
    assert!(
        text.contains("0483:3754:0123456789abcdef01234567  STLink V3"),
        "{text}"
    );
}

#[test]
fn a_probe_that_matches_several_boards_lists_only_those() {
    let boards = boards();
    let error = select(&boards, Some("probe://303A:1001")).unwrap_err();
    assert_eq!(
        error,
        Error::ManyBoards {
            selector: Some("probe://303A:1001".to_owned()),
            boards: boards[..2].to_vec(),
        }
    );
    let text = error.to_string();
    assert!(
        text.starts_with("probe://303A:1001 matches 2 boards."),
        "{text}"
    );
    assert!(!text.contains("0483"), "{text}");
}

/// A bare word names an entry in a devices file, never a probe, even when it is part of a probe's selector.
#[test]
fn a_bare_word_is_a_device_name_and_never_matches_a_probe() {
    let boards = boards();
    let error = select(&boards, Some("dd:ee:01")).unwrap_err();
    assert_eq!(
        error,
        Error::NoSuchDevice {
            device: "dd:ee:01".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "no device named dd:ee:01: there is no devices file yet. A probe is probe://<part of its selector>"
    );
}

#[test]
fn a_device_of_another_kind_is_refused_even_when_one_board_is_attached() {
    let one = vec![board("0483:3754:0123", "STLink V3")];
    for device in ["tcp://10.0.0.5:2323", "PROBE://0483"] {
        let error = select(&one, Some(device)).unwrap_err();
        assert_eq!(
            error,
            Error::UnsupportedDevice {
                device: device.to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            format!("{device}: this kind of device is not supported yet")
        );
    }
}

#[test]
fn no_board_at_all_says_so_without_a_list() {
    assert_eq!(
        select(&[], None).unwrap_err().to_string(),
        "no board is attached."
    );
}

#[test]
fn a_board_listed_in_an_error_shows_its_chip_when_known() {
    let mut boards = boards();
    boards[0].chip = Some("esp32c5".to_owned());
    let text = select(&boards, None).unwrap_err().to_string();
    assert_eq!(
        text,
        "3 boards are attached. Name one with probe://<part of its selector>:\n  \
         303a:1001:aa:bb:cc:dd:ee:01  ESP JTAG  esp32c5\n  \
         303a:1001:12:34:56:78:9a:02  ESP JTAG\n  \
         0483:3754:0123456789abcdef01234567  STLink V3"
    );
}
