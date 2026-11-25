//! The console of `console_async` with `mcu-console-net`'s `net` commands over Ethernet, for the Ethernet scenario tests.
//! Only the ESP32-S31 board crate builds it, because only that board has Ethernet.

#![no_std]
#![no_main]

mod common;
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/board.rs"));

use board::{boot_count, embassy, ethernet, image_id, info, init_psram_250mhz, psram, reset};
use common::{Echo, FIRMWARE};
use embassy_executor::Spawner;
use embassy_time::Delay;
use esp_hal::ethernet::EthernetDmaStorage;
use mcu_console::Config;
use mcu_console_net::NetCommands;
use static_cell::ConstStaticCell;

/// The Ethernet descriptor rings, 45 receive and 45 transmit slots. An application tunes throughput here: the transmit
/// ring's length is the driver's burst size, which caps the TCP window smoltcp advertises.
static RINGS: ConstStaticCell<EthernetDmaStorage<45, 45>> =
    ConstStaticCell::new(EthernetDmaStorage::new());

/// `net iperf`'s socket receive and send buffers, and the client's work buffer, at the board's sizes.
#[cfg(feature = "iperf")]
static IPERF: ConstStaticCell<([u8; IPERF_SOCKET], [u8; IPERF_SOCKET], [u8; IPERF_WORK])> =
    ConstStaticCell::new(([0; IPERF_SOCKET], [0; IPERF_SOCKET], [0; IPERF_WORK]));

/// `lock <n>`, which takes the board's timed mutex `n` times, so the scenario tests see `perf_locks` count them.
#[cfg(feature = "perf")]
mod lock {
    use super::board::perf::TimedRawMutex;
    use core::cell::Cell;
    use embassy_sync::blocking_mutex::Mutex;
    use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};

    static HELP: [Help; 1] = [Help {
        usage: "lock <n>",
        summary: "take the timed mutex n times",
        secret: false,
    }];

    static MUTEX: Mutex<TimedRawMutex, Cell<u32>> = Mutex::new(Cell::new(0));

    pub struct Lock;

    impl Commands for Lock {
        fn help(&self) -> impl Iterator<Item = &'static Help> {
            HELP.iter()
        }

        async fn run<R: Reply>(
            &mut self,
            name: &str,
            mut args: Args<'_>,
            _: &mut R,
            _: Cancel<'_>,
        ) -> CommandResult {
            if name != "lock" {
                return CommandResult::Unknown;
            }
            let Some(times) = args.next().and_then(|times| times.parse::<u32>().ok()) else {
                return CommandResult::Err("usage: lock <n>");
            };
            for _ in 0..times {
                MUTEX.lock(|taken| taken.set(taken.get().wrapping_add(1)));
            }
            CommandResult::Ok
        }
    }
}

#[esp_hal::main]
async fn main(spawner: Spawner) {
    // At 250 MHz PSRAM, the MPLL lets RGMII run at 1 Gbit/s.
    let mut peripherals = init_psram_250mhz();
    let boot = boot_count();
    embassy::start(peripherals.TIMG0);
    let psram = psram(peripherals.PSRAM);
    let info = info(FIRMWARE, peripherals.FLASH.reborrow(), &psram);
    let pins = ethernet::Pins {
        mdc: peripherals.GPIO5,
        mdio: peripherals.GPIO6,
        phy_reset: peripherals.GPIO7,
        txd0: peripherals.GPIO8,
        txd1: peripherals.GPIO9,
        txd2: peripherals.GPIO10,
        txd3: peripherals.GPIO11,
        tx_ctl: peripherals.GPIO12,
        tx_clk: peripherals.GPIO13,
        rx_clk: peripherals.GPIO14,
        rx_ctl: peripherals.GPIO15,
        rxd3: peripherals.GPIO16,
        rxd2: peripherals.GPIO17,
        rxd1: peripherals.GPIO18,
        rxd0: peripherals.GPIO19,
    };
    let Ok((stack, capabilities)) =
        ethernet::start(spawner, peripherals.ETH, pins, RINGS.take()).await
    else {
        defmt::panic!("Ethernet did not start");
    };
    let net = NetCommands::ethernet(stack, capabilities.max_transmission_unit);
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
    let commands = (info, (net, Echo));
    #[cfg(feature = "perf")]
    let commands = mcu_console_perf::Perf::new(board::perf::EspPlatform, (commands, lock::Lock));
    let mut console: mcu_console::asynch::Console<_, _, _, _, 256> =
        mcu_console::asynch::Console::new(rx, tx, Delay, commands, config);
    let stop = console.run().await;
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}
