//! The embedded-devkit test platform for the ESP32-C5-DevKitC-1 v1.2.
//!
//! It is the ESP32 family crate, `embedded-devkit-esp`, for the ESP32-C5, with what is particular to this board:
//! its PSRAM settings and its name in `info`.

#![no_std]

#[cfg(feature = "embassy")]
pub use embedded_devkit_esp::embassy;
#[cfg(feature = "wifi")]
pub use embedded_devkit_esp::wifi;
pub use embedded_devkit_esp::{
    Info, UsbTx, boot_count, console, image_id, init, reset, reset_name,
};

use esp_hal::peripherals::{FLASH, PSRAM};
use esp_hal::psram::{Psram, PsramConfig};

/// Starts PSRAM with this board's settings. `Psram::new` also sets the flash clock, to the 80 MHz the module's
/// flash is rated for.
pub fn psram(psram: PSRAM<'static>) -> Psram {
    Psram::new(psram, PsramConfig::default())
}

/// The `info` command (B-10) for this board and `firmware`, its name and version, such as
/// `my-app 0.1.0`. Reads the flash capacity once through `flash`, which the caller lends with
/// `reborrow()`, and the PSRAM size from `psram`.
pub fn info(firmware: &'static str, flash: FLASH<'_>, psram: &Psram) -> Info {
    Info::new(env!("CARGO_PKG_NAME"), firmware, flash, psram)
}
