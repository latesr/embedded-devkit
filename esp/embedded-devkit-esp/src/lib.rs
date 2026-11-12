//! What the embedded-devkit ESP32 board crates share. A board crate turns on one chip feature, such as `esp32c5`,
//! and adds what it knows about its board.
//!
//! It brings up the chip, counts boots, and gives the console its transport over the chip's own USB port, USB
//! Serial/JTAG. It also provides what every application for an ESP32 board needs: the ESP-IDF app descriptor the
//! bootloader reads, the semihosting panic handler, and defmt over RTT.

#![no_std]

#[cfg(feature = "wifi")]
extern crate alloc;

// esp-radio needs a heap, and esp-alloc's global allocator links only when something uses the crate.
#[cfg(feature = "wifi")]
use esp_alloc as _;

#[cfg(feature = "wifi")]
pub mod wifi;

#[cfg(all(feature = "ethernet", feature = "esp32s31"))]
pub mod ethernet;

#[cfg(all(feature = "perf", feature = "esp32s31"))]
pub mod perf;

use core::fmt::{self, Display, Formatter};
use defmt_rtt as _;
use embedded_io::{ErrorType, Write, WriteReady};
use esp_hal::clock::CpuClock;
use esp_hal::flash::{self, Flash};
use esp_hal::peripherals::{FLASH, Peripherals, USB_DEVICE};
use esp_hal::psram::Psram;
use esp_hal::rtc_cntl::SocResetReason;
use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::{Blocking, ram};
use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};
use semihosting as _;

esp_bootloader_esp_idf::esp_app_desc!();

/// Brings up the chip at its fastest CPU clock and returns every peripheral. The application passes on the ones the
/// board crate's other functions take, and keeps the rest.
pub fn init() -> Peripherals {
    esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()))
}

/// How many times the firmware has booted since the last reset that was not a software reset.
#[ram(unstable(rtc_fast, persistent))]
static mut BOOT_COUNT: u32 = 0;

/// Counts this boot and returns the count. The count restarts at 1 after any reset that is not a software
/// reset, because RTC RAM holds random bytes after power-on. Call it once per boot, before the console starts.
pub fn boot_count() -> u32 {
    let software = reset_name(esp_hal::system::reset_reason()) == "software";
    critical_section::with(|_| {
        // SAFETY: nothing else touches BOOT_COUNT. esp-hal's critical section masks interrupts on this core, and on
        // a chip with two cores it also holds a lock that the other core waits on, so this is the only access. A
        // persistent static must be `AnyBitPattern`, which rules out an atomic or a mutex.
        #[expect(unsafe_code)]
        unsafe {
            let count = if software {
                BOOT_COUNT.wrapping_add(1)
            } else {
                1
            };
            BOOT_COUNT = count;
            count
        }
    })
}

/// The chip's name in `info`.
#[cfg(feature = "esp32c5")]
const CHIP: &str = "esp32c5";

/// The name B-12 gives a reset reason: `power-on`, `software`, `watchdog`, `brown-out`, `debugger`, or `other`
/// followed by the chip's own name for the reason in brackets. It is plain `other` when the chip gives no reason.
/// The C5 never gives `pin`, because its EN pin resets the chip as a power-on does.
#[cfg(feature = "esp32c5")]
pub fn reset_name(reason: Option<SocResetReason>) -> &'static str {
    use SocResetReason::*;
    match reason {
        Some(ChipPowerOn) => "power-on",
        Some(CoreSw | Cpu0Sw) => "software",
        Some(
            CoreMwdt0 | CoreMwdt1 | CoreRtcWdt | Cpu0Mwdt0 | Cpu0Mwdt1 | Cpu0RtcWdt | SysRtcWdt
            | SysSuperWdt,
        ) => "watchdog",
        Some(SysBrownOut) => "brown-out",
        Some(CoreUsbJtag | Cpu0JtagCpu) => "debugger",
        Some(CoreDeepSleep) => "other (CoreDeepSleep)",
        Some(CoreEfuseCrc) => "other (CoreEfuseCrc)",
        Some(CoreUsbUart) => "other (CoreUsbUart)",
        Some(PowerGlitch) => "other (PowerGlitch)",
        Some(CpuLockup) => "other (CpuLockup)",
        None => "other",
    }
}

/// The chip's name in `info`.
#[cfg(feature = "esp32s31")]
const CHIP: &str = "esp32s31";

/// The name B-12 gives a reset reason: `power-on`, `software`, `watchdog`, `brown-out`, `debugger`, or `other`
/// followed by the chip's own name for the reason in brackets. It is plain `other` when the chip gives no reason.
#[cfg(feature = "esp32s31")]
pub fn reset_name(reason: Option<SocResetReason>) -> &'static str {
    use SocResetReason::*;
    match reason {
        Some(ChipPowerOn) => "power-on",
        Some(CoreSw | CpuSw) => "software",
        Some(CoreMwdt0 | CoreMwdt1 | CoreRwdt | CpuMwdt | CpuRwdt | SysRwdt | SysSuperWdt) => {
            "watchdog"
        }
        Some(SysBrownOut) => "brown-out",
        Some(CoreUsbJtag | CpuJtag) => "debugger",
        Some(CoreDeepSleep) => "other (CoreDeepSleep)",
        Some(CpuPmuPwrDown) => "other (CpuPmuPwrDown)",
        Some(CorePwrGlitch) => "other (CorePwrGlitch)",
        Some(CoreEfuseCrc) => "other (CoreEfuseCrc)",
        Some(CoreUsbUart) => "other (CoreUsbUart)",
        Some(CpuLockup) => "other (CpuLockup)",
        None => "other",
    }
}

/// The `info` command (B-10): eight `key: value` lines that describe the board and the application.
pub struct Info {
    board: &'static str,
    firmware: &'static str,
    flash: usize,
    psram: usize,
}

static INFO_HELP: [Help; 1] = [Help {
    usage: "info",
    summary: "describe the board",
    secret: false,
}];

impl Info {
    /// The `info` command for the board crate named `board`, and for `firmware`, its name and version, such as
    /// `my-app 0.1.0`. Reads the flash capacity once through `flash`, which the caller lends with
    /// `reborrow()`, and the PSRAM size from `psram`.
    pub fn new(
        board: &'static str,
        firmware: &'static str,
        flash: FLASH<'_>,
        psram: &Psram,
    ) -> Self {
        let flash = Flash::new(flash, flash::Config::default()).map_or(0, |flash| flash.capacity());
        Self {
            board,
            firmware,
            flash,
            psram: psram.size(),
        }
    }
}

impl Commands for Info {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        INFO_HELP.iter()
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        _: Args<'_>,
        reply: &mut R,
        _: Cancel<'_>,
    ) -> CommandResult {
        if name != "info" {
            return CommandResult::Unknown;
        }
        let revision = esp_hal::efuse::chip_revision();
        reply
            .field(
                "board",
                self.board
                    .strip_prefix("embedded-devkit-")
                    .unwrap_or(self.board),
            )
            .await;
        reply
            .field(
                "chip",
                format_args!("{CHIP} v{}.{}", revision.major, revision.minor),
            )
            .await;
        reply.field("id", esp_hal::efuse::base_mac_address()).await;
        reply.field("flash", Size(self.flash)).await;
        reply.field("psram", Size(self.psram)).await;
        reply
            .field(
                "cpu",
                format_args!("{} MHz", esp_hal::clock::cpu_clock().as_mhz()),
            )
            .await;
        reply
            .field("reset", reset_name(esp_hal::system::reset_reason()))
            .await;
        reply.field("firmware", self.firmware).await;
        CommandResult::Ok
    }
}

/// A size in bytes, written in whole megabytes as the datasheets write it, or `none` for zero.
struct Size(usize);

impl Display for Size {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            f.write_str("none")
        } else {
            write!(f, "{} MB", self.0 / 1_048_576)
        }
    }
}

/// Embassy on esp-rtos, and the console's async transport (B-3, B-4).
#[cfg(feature = "embassy")]
pub mod embassy {
    use esp_hal::Async;
    use esp_hal::peripherals::{TIMG0, USB_DEVICE};
    use esp_hal::timer::timg::TimerGroup;
    use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};

    /// Starts esp-rtos on the first timer of `timg0`. esp-rtos also drives Embassy's time. Call it once, before
    /// the first `await`.
    pub fn start(timg0: TIMG0<'static>) {
        esp_rtos::start(TimerGroup::new(timg0).timer0);
    }

    /// The console transport over USB Serial/JTAG, for the async console. The async console bounds each write
    /// with its own timeout, so a transmitter that waits for the host does not block it for ever.
    pub fn console(
        usb: USB_DEVICE<'static>,
    ) -> (
        UsbSerialJtagRx<'static, Async>,
        UsbSerialJtagTx<'static, Async>,
    ) {
        UsbSerialJtag::new(usb).into_async().split()
    }
}

/// Resets the chip, for the console's `reset` command.
pub fn reset() -> ! {
    esp_hal::system::software_reset()
}

/// The ID of the running image, for the console's [`Config::image`](mcu_console::Config::image): the SHA-256 of
/// its ELF file, which espflash writes into the app descriptor. `None` when the descriptor holds only zeros, as
/// an image that espflash did not make does. The firmware must link the board crate's `mcu-run.x`, which names
/// the ID `__mcu_image_id` (B-24).
pub fn image_id() -> Option<&'static [u8]> {
    #[expect(unsafe_code)]
    unsafe extern "C" {
        static __mcu_image_id: [u8; 32];
    }
    // SAFETY: `mcu-run.x` puts `__mcu_image_id` on the 32 bytes of `app_elf_sha256` in the app descriptor, and
    // its ASSERT fails the link when the descriptor is not ESP-IDF's. The descriptor is read-only data, which
    // nothing writes while the firmware runs.
    #[expect(unsafe_code)]
    let id = unsafe { &__mcu_image_id };
    id.iter().any(|&byte| byte != 0).then_some(id.as_slice())
}

/// The console transport over USB Serial/JTAG: the receiver, and a transmitter that never waits for the host.
pub fn console(usb: USB_DEVICE<'static>) -> (UsbSerialJtagRx<'static, Blocking>, UsbTx) {
    let (rx, tx) = UsbSerialJtag::new(usb).split();
    (rx, UsbTx(tx))
}

/// The USB Serial/JTAG transmitter as a transport that never blocks when no host reads. esp-hal's own `Write`
/// waits for the host to drain the FIFO, and with no host attached it waits for ever.
pub struct UsbTx(UsbSerialJtagTx<'static, Blocking>);

impl ErrorType for UsbTx {
    type Error = embedded_io::ErrorKind;
}

impl WriteReady for UsbTx {
    /// True when the FIFO has room for a byte, so that `write` writes at least one without waiting.
    fn write_ready(&mut self) -> Result<bool, Self::Error> {
        Ok(self.0.write_ready())
    }
}

impl Write for UsbTx {
    /// Writes as many bytes as the FIFO takes, at least one when the FIFO has room.
    /// Fails with `WriteZero` when the FIFO has no room, because embedded-io forbids `Ok(0)` for a non-empty
    /// buffer.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        let written = bytes
            .iter()
            .take_while(|&&byte| self.0.write_byte_nb(byte).is_ok())
            .count();
        if written == 0 && !bytes.is_empty() {
            return Err(embedded_io::ErrorKind::WriteZero);
        }
        Ok(written)
    }

    /// Hands the FIFO to the host without waiting for the host to take it.
    fn flush(&mut self) -> Result<(), Self::Error> {
        let _ = self.0.flush_tx_nb();
        Ok(())
    }
}
