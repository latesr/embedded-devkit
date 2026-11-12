//! Writes ten defmt lines, one every half second, and exits through semihosting with code 3. `mcu-run` stops
//! on a line, at a deadline or with the code, so one binary proves all three (M-9).

#![no_std]
#![no_main]

use embassy_time::{Duration, block_for};
use embedded_devkit_nucleo_h7s3l8::{boot_count, init};

#[cortex_m_rt::entry]
fn main() -> ! {
    let _peripherals = init();
    defmt::info!("exit firmware, boot {}", boot_count());
    for tick in 1..=10 {
        defmt::info!("tick {}", tick);
        block_for(Duration::from_millis(500));
    }
    defmt::info!("exiting with code 3");
    semihosting::process::exit(3)
}
