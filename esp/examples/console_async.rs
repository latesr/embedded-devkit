//! The console of the `console` example, run async under Embassy on esp-rtos, for the scenario tests.

#![no_std]
#![no_main]

mod common;
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/board.rs"));

use board::{boot_count, embassy, image_id, info, init, psram, reset};
use common::{Echo, FIRMWARE};
use embassy_executor::Spawner;
use embassy_time::Delay;
use mcu_console::Config;

#[esp_hal::main]
async fn main(_spawner: Spawner) {
    let mut peripherals = init();
    let boot = boot_count();
    embassy::start(peripherals.TIMG0);
    let psram = psram(peripherals.PSRAM);
    let info = info(FIRMWARE, peripherals.FLASH.reborrow(), &psram);
    defmt::info!("console firmware, boot {}", boot);
    let (rx, tx) = embassy::console(peripherals.USB_DEVICE);
    let config = Config {
        name: NAME,
        boot,
        write_timeout_ms: 500,
        reset,
        image: image_id(),
    };
    let mut console: mcu_console::asynch::Console<_, _, _, _, 128> =
        mcu_console::asynch::Console::new(rx, tx, Delay, (info, Echo), config);
    let stop = console.run().await;
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}
