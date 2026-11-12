//! What the timing test needs from a firmware: the bootloader's app descriptor, defmt over RTT, the semihosting
//! panic handler, and the chip brought up as the ESP32-S31 board crate brings it up.

#![no_std]

use defmt_rtt as _;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::Peripherals;
use semihosting as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Brings up the chip at its fastest CPU clock, as the board crate's `init` does.
pub fn init() -> Peripherals {
    esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()))
}
