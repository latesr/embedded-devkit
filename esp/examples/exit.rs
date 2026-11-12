//! Writes ten defmt lines, one every half second, and exits through semihosting with code 3. `mcu-run` stops
//! on a line, at a deadline or with the code, so one binary proves all three (M-9).

#![no_std]
#![no_main]

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/board.rs"));

use board::{boot_count, init};
use esp_hal::delay::Delay;

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = init();
    defmt::info!("exit firmware, boot {}", boot_count());
    let delay = Delay::new();
    for tick in 1..=10 {
        defmt::info!("tick {}", tick);
        delay.delay_millis(500);
    }
    defmt::info!("exiting with code 3");
    semihosting::process::exit(3)
}
