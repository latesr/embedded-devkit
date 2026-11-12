//! A board crate's `console` and `console_async` examples, driven through `mcu-scenario` (H-3).
//! `console_async` runs the same console under Embassy (B-3, B-4).
//!
//! Each test starts an example through `mcu-scenario`, which resets it through its console or flashes it, then
//! talks to the console over the board's USB serial port. `cargo xtask hil` sets `MCU_BOARD` to the board crate.
//! `MCU_DEVICE` names the board when two attached boards have the board crate's chip.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use common::{Id, TIMEOUT, board, board_crate, console, field, firmware, fresh_console};
use mcu_run::{Client, CommandResult, Image, Runner};
use std::time::Duration;

#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn echo_writes_its_text_back() {
    let mut client = console("console");
    let reply = client.send("echo hi there").unwrap();
    assert_eq!(reply.body, ["hi there"]);
    assert_eq!(reply.result, CommandResult::Ok);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn help_lists_the_built_in_commands_then_the_firmware_s() {
    let mut client = console("console");
    let reply = client.send("help").unwrap();
    assert_eq!(
        reply.body,
        [
            "help: list the commands",
            "reset: reset the chip",
            "info: describe the board",
            "echo <text>: write the text back",
        ]
    );
    assert_eq!(reply.result, CommandResult::Ok);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn an_unknown_command_is_named_in_the_error() {
    let mut client = console("console");
    let reply = client.send("frob").unwrap();
    assert_eq!(reply.body, Vec::<String>::new());
    assert_eq!(
        reply.result,
        CommandResult::Err("unknown command frob".to_owned())
    );
}

/// The lines the board crate sets match exactly, the ID matches the board, and the lines that vary between units
/// have their form (H-6). The flash ends with a probe reset, which B-12 names `debugger`.
#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn info_describes_the_board_and_its_unit() {
    let board_crate = board_crate();
    let mut client = fresh_console("console");
    let reply = client.send("info").unwrap();
    assert_eq!(reply.result, CommandResult::Ok);
    let keys: Vec<&str> = reply
        .body
        .iter()
        .map(|line| line.split_once(": ").unwrap().0)
        .collect();
    assert_eq!(
        keys,
        [
            "board", "chip", "id", "flash", "psram", "cpu", "reset", "firmware"
        ]
    );
    assert_eq!(reply.body[0], board_crate.board);
    let revision = reply.body[1]
        .strip_prefix(&format!("chip: {} ", board_crate.chip))
        .unwrap_or_else(|| panic!("{:?} names another chip", reply.body[1]));
    assert!(
        !revision.is_empty() && !revision.contains(' ') || revision.starts_with("rev "),
        "{revision:?} is not one revision"
    );
    let id = field(&reply, "id");
    match board_crate.id {
        Id::UsbSerial => assert_eq!(id, board().serial().to_lowercase()),
        Id::Hex(digits) => assert!(
            id.len() == digits && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{id:?} is not {digits} hex digits"
        ),
    }
    let size = |text: &str| {
        text.split_once(' ')
            .is_some_and(|(n, unit)| n.parse::<u32>().is_ok() && (unit == "KB" || unit == "MB"))
    };
    let flash = field(&reply, "flash");
    let (internal, external) = flash.split_once(" + ").unwrap_or((flash, "0 MB external"));
    assert!(
        size(internal) && external.strip_suffix(" external").is_some_and(size),
        "{flash:?} is not a flash size"
    );
    let psram = field(&reply, "psram");
    assert!(
        psram == "none" || size(psram),
        "{psram:?} is not a PSRAM size"
    );
    assert_eq!(reply.body[5], board_crate.cpu);
    assert_eq!(reply.body[6], "reset: debugger");
    assert_eq!(reply.body[7], board_crate.firmware());
}

/// H-5.
#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn info_reports_a_software_reset_after_the_reset_command() {
    let mut client = console("console");
    assert_eq!(client.send("reset").unwrap().result, CommandResult::Ok);
    client.wait_for_banner(TIMEOUT).unwrap();
    let reply = client.send("info").unwrap();
    assert_eq!(reply.body[6], "reset: software");
}

/// A software reset keeps the USB link, so the client reads each banner on the port it already has open.
#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn each_reset_restarts_the_firmware_with_the_next_boot_count() {
    let mut client = console("console");
    let mut banners = Vec::new();
    for _ in 0..2 {
        let reply = client.send("reset").unwrap();
        assert_eq!(reply.result, CommandResult::Ok);
        banners.push(client.wait_for_banner(TIMEOUT).unwrap());
    }
    assert_eq!(banners[0].name, board_crate().prompt);
    assert_eq!(banners[1].name, board_crate().prompt);
    assert_eq!(banners[1].boot, banners[0].boot + 1);
    assert_eq!(banners[1].protocol, Some(mcu_run::PROTOCOL));
}

/// The firmware reads its own image ID from flash, and `mcu-run` reads the same ID from the ELF (K-26, M-25,
/// B-24).
#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn the_banner_names_the_image_that_was_flashed() {
    let elf = firmware("console").elf_path().unwrap();
    let id = Image::from_elf(elf).unwrap().id().unwrap();
    assert!(id.is_some(), "the ELF has no image ID");
    let mut client = console("console");
    assert_eq!(client.send("reset").unwrap().result, CommandResult::Ok);
    assert_eq!(client.wait_for_banner(TIMEOUT).unwrap().image, id);
}

#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn the_async_console_answers_echo_info_and_an_unknown_command() {
    let mut client = console("console_async");
    let echo = client.send("echo hi there").unwrap();
    assert_eq!(echo.body, ["hi there"]);
    assert_eq!(echo.result, CommandResult::Ok);
    let info = client.send("info").unwrap();
    assert_eq!(info.body[0], board_crate().board);
    assert_eq!(info.body[7], board_crate().firmware());
    let unknown = client.send("frob").unwrap();
    assert_eq!(
        unknown.result,
        CommandResult::Err("unknown command frob".to_owned())
    );
}

/// A run follows the board's log across a software reset, onto the next boot's lines (M-18). It drives
/// `mcu-run` directly, because the run is what it tests.
#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn a_run_follows_the_log_across_a_reset() {
    let elf = firmware("console").elf_path().unwrap();
    let (runner, board) = (Runner::new(Image::from_elf(elf).unwrap()).unwrap(), board());
    // The port is open before the flash, so the client reads the banner the new firmware writes when it boots.
    // Before that banner, whatever firmware the board held last would answer.
    let mut client = Client::new(mcu_run::open_port(&mcu_run::port(&board).unwrap()).unwrap());
    let following = board.clone();
    let log = std::env::temp_dir().join(format!("mcu-run-follow-{}.log", std::process::id()));
    let log_path = log.clone();
    let run = std::thread::spawn(move || {
        runner.run(
            &following,
            Some("console firmware, boot 2"),
            Some(Duration::from_secs(30)),
            Some(&log_path),
        )
    });
    assert_eq!(
        client
            .wait_for_banner(Duration::from_secs(20))
            .unwrap()
            .boot,
        1
    );
    // Long enough for the run to read the first boot's lines, so the reset comes while it follows them.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        client.send("reset", TIMEOUT).unwrap().result,
        CommandResult::Ok
    );
    assert_eq!(run.join().unwrap(), Ok(mcu_run::Stop::Matched));
    let text = std::fs::read_to_string(&log).unwrap();
    let _ = std::fs::remove_file(&log);
    let at = |line: &str| {
        text.find(line)
            .unwrap_or_else(|| panic!("no {line:?} in the run's log:\n{text}"))
    };
    assert!(at("console firmware, boot 1") < at("(the board reset)"));
    assert!(at("(the board reset)") < at("console firmware, boot 2"));
}

#[test]
#[ignore = "needs the board that MCU_BOARD names"]
fn the_async_console_resets_with_the_next_boot_count() {
    let mut client = console("console_async");
    let mut banners = Vec::new();
    for _ in 0..2 {
        assert_eq!(client.send("reset").unwrap().result, CommandResult::Ok);
        banners.push(client.wait_for_banner(TIMEOUT).unwrap());
    }
    assert_eq!(banners[1].boot, banners[0].boot + 1);
}
