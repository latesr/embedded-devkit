//! The console over USB Serial/JTAG, with the board's `info` command and an `echo` command, for `mcu console`
//! and the scenario tests.

#![no_std]
#![no_main]

mod common;
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/board.rs"));

use board::{boot_count, console, image_id, info, init, psram, reset};
use common::{Echo, FIRMWARE};
use esp_hal::delay::Delay;
use mcu_console::Config;

#[esp_hal::main]
fn main() -> ! {
    let mut peripherals = init();
    let boot = boot_count();
    let psram = psram(peripherals.PSRAM);
    let info = info(FIRMWARE, peripherals.FLASH.reborrow(), &psram);
    defmt::info!("console firmware, boot {}", boot);
    let (rx, tx) = console(peripherals.USB_DEVICE);
    let config = Config {
        name: NAME,
        boot,
        write_timeout_ms: 500,
        reset,
        image: image_id(),
    };
    let mut console: mcu_console::blocking::Console<_, _, _, _, 128> =
        mcu_console::blocking::Console::new(rx, tx, Delay::new(), (info, Echo), config);
    let stop = console.run();
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}
