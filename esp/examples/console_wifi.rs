//! The console of `console_async` with `mcu-console-net`'s `net` commands over Wi-Fi, for the Wi-Fi scenario tests.

#![no_std]
#![no_main]

mod common;
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/board.rs"));

use board::{boot_count, embassy, image_id, info, init, psram, reset, wifi};
use common::{Echo, FIRMWARE};
use embassy_executor::Spawner;
use embassy_time::Delay;
use mcu_console::Config;
use mcu_console_net::NetCommands;
#[cfg(feature = "iperf")]
use static_cell::ConstStaticCell;

/// `net iperf`'s socket receive and send buffers, and the client's work buffer, at the board's sizes.
#[cfg(feature = "iperf")]
static IPERF: ConstStaticCell<([u8; IPERF_SOCKET], [u8; IPERF_SOCKET], [u8; IPERF_WORK])> =
    ConstStaticCell::new(([0; IPERF_SOCKET], [0; IPERF_SOCKET], [0; IPERF_WORK]));

#[esp_hal::main]
async fn main(spawner: Spawner) {
    let mut peripherals = init();
    let boot = boot_count();
    embassy::start(peripherals.TIMG0);
    let psram = psram(peripherals.PSRAM);
    let info = info(FIRMWARE, peripherals.FLASH.reborrow(), &psram);
    // An application tunes Wi-Fi throughput here, with esp-radio's settings and with
    // `ESP_RADIO_CONFIG_WIFI_MAX_BURST_SIZE` at build time. A queued frame holds one of the driver's dynamic
    // receive buffers, so a longer receive queue needs more of them, and more heap: see `wifi::start`.
    let config = wifi::ControllerConfig::default();
    let Ok((stack, station)) = wifi::start(spawner, peripherals.WIFI, config) else {
        defmt::panic!("the radio did not start");
    };
    let net = NetCommands::wifi(stack, station.mtu(), station);
    #[cfg(feature = "iperf")]
    let net = {
        let (rx, tx, work) = IPERF.take();
        net.with_iperf(rx, tx, work)
    };
    defmt::info!("console firmware, boot {}", boot);
    let (dp, dm) = usb_pins!(peripherals);
    let Ok((rx, tx)) = embassy::console(peripherals.USB_DEVICE, dp, dm) else {
        defmt::panic!("USB Serial/JTAG did not start");
    };
    let config = Config {
        name: NAME,
        boot,
        write_timeout_ms: 500,
        reset,
        image: image_id(),
    };
    let mut console: mcu_console::asynch::Console<_, _, _, _, 256> =
        mcu_console::asynch::Console::new(rx, tx, Delay, (info, (net, Echo)), config);
    let stop = console.run().await;
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}
