//! The embedded-devkit test platform for the ESP32-S31-Function-CoreBoard-1 v1.0.
//!
//! It is the ESP32 family crate, `embedded-devkit-esp`, for the ESP32-S31, with what is particular to this board:
//! its PSRAM settings, its name in `info`, and its Ethernet.

#![no_std]

#[cfg(feature = "ethernet")]
pub mod ethernet;

#[cfg(feature = "embassy")]
pub use embedded_devkit_esp::embassy;
#[cfg(feature = "perf")]
pub use embedded_devkit_esp::perf;
#[cfg(feature = "wifi")]
pub use embedded_devkit_esp::wifi;
pub use embedded_devkit_esp::{Info, UsbTx, boot_count, console, image_id, reset, reset_name};

use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{FLASH, PSRAM, Peripherals};
use esp_hal::psram::{Psram, PsramConfig};

/// Brings up the chip at its fastest CPU clock, and returns every peripheral. The application passes on the ones
/// the board crate's other functions take, and keeps the rest.
///
/// The MPLL runs at twice the PSRAM frequency that the application's `ESP_HAL_CONFIG_PSRAM_FREQUENCY` sets: 400 MHz
/// for 200 MHz, or 500 MHz for 250 MHz. At 400 MHz, RGMII Ethernet runs at 100 Mbit/s at most.
pub fn init() -> Peripherals {
    esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()))
}

/// Takes the module's Octal SPI PSRAM, which the start-up has started at the application's
/// `ESP_HAL_CONFIG_PSRAM_FREQUENCY`, and detects its size. 200 MHz is the rating in the module's datasheet, and
/// 250 MHz runs the PSRAM beyond it.
pub fn psram(psram: PSRAM<'static>) -> Psram {
    Psram::new(psram, PsramConfig::default())
}

/// The `info` command (B-10) for this board and `firmware`, its name and version, such as
/// `my-app 0.1.0`. Reads the flash capacity once through `flash`, which the caller lends with
/// `reborrow()`, and the PSRAM size from `psram`.
pub fn info(firmware: &'static str, flash: FLASH<'_>, psram: &Psram) -> Info {
    Info::new(env!("CARGO_PKG_NAME"), firmware, flash, psram)
}
