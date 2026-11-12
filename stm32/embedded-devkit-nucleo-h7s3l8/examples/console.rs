//! The console over the board's virtual COM port, with the board's `info` command and an `echo` command, for
//! `mcu console` and the scenario tests.

#![no_std]
#![no_main]

mod common;

use common::{Echo, FIRMWARE};
use embassy_time::Delay;
use embedded_devkit_nucleo_h7s3l8::{boot_count, console, image_id, info, init, reset};
use mcu_console::Config;

#[cortex_m_rt::entry]
fn main() -> ! {
    let peripherals = init();
    let boot = boot_count();
    defmt::info!("console firmware, boot {}", boot);
    let (rx, tx) = match console(peripherals.USART3, peripherals.PD9, peripherals.PD8) {
        Ok(halves) => halves,
        Err(error) => defmt::panic!("USART3: {}", error),
    };
    let config = Config {
        name: "nucleo",
        boot,
        write_timeout_ms: 500,
        reset,
        image: image_id(),
    };
    let mut console: mcu_console::blocking::Console<_, _, _, _, 128> =
        mcu_console::blocking::Console::new(rx, tx, Delay, (info(FIRMWARE), Echo), config);
    let stop = console.run();
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}
