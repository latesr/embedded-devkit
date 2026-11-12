//! The console of the `console` example, run async on the low interrupt executor, for the scenario tests. `main`
//! idles while the executors do the work, as in the async test environment (H-1).

#![no_std]
#![no_main]

mod common;

use common::{Echo, FIRMWARE};
use embassy_stm32::usart::{BufferedUartRx, BufferedUartTx};
use embassy_time::Delay;
use embedded_devkit_nucleo_h7s3l8::embassy::{self, ConsoleBuffers};
use embedded_devkit_nucleo_h7s3l8::{boot_count, image_id, info, init, reset};
use mcu_console::Config;
use static_cell::StaticCell;

embedded_devkit_nucleo_h7s3l8::interrupt_executors!();

static BUFFERS: StaticCell<ConsoleBuffers> = StaticCell::new();

#[embassy_executor::task]
async fn console(rx: BufferedUartRx<'static>, tx: BufferedUartTx<'static>, boot: u32) {
    let config = Config {
        name: "nucleo",
        boot,
        write_timeout_ms: 500,
        reset,
        image: image_id(),
    };
    let mut console: mcu_console::asynch::Console<_, _, _, _, 128> =
        mcu_console::asynch::Console::new(rx, tx, Delay, (info(FIRMWARE), Echo), config);
    let stop = console.run().await;
    defmt::panic!("the console stopped: {}", defmt::Debug2Format(&stop));
}

#[cortex_m_rt::entry]
fn main() -> ! {
    let peripherals = init();
    let boot = boot_count();
    defmt::info!("console firmware, boot {}", boot);
    let (_high, low) = start_executors();
    let buffers = BUFFERS.init(ConsoleBuffers::new());
    let (rx, tx) = match embassy::console(
        peripherals.USART3,
        peripherals.PD9,
        peripherals.PD8,
        buffers,
    ) {
        Ok(halves) => halves,
        Err(error) => defmt::panic!("USART3: {}", error),
    };
    match console(rx, tx, boot) {
        Ok(task) => low.spawn(task),
        Err(error) => defmt::panic!("the console task: {}", error),
    }
    loop {
        cortex_m::asm::wfi();
    }
}
