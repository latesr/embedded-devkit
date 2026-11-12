//! The embedded-devkit test platform for the NUCLEO-H7S3L8 (B-1).
//!
//! The chip boots from internal flash, whose start-up code maps the external flash, where the program lives and
//! executes in place (B-14). The crate brings up the chip, counts boots, and gives the console its transport over
//! USART3, which the board's ST-LINK carries as a virtual COM port. It also provides what every application for the board
//! needs: the semihosting panic handler, and defmt over RTT.

#![no_std]

mod boot;
#[cfg(feature = "embassy")]
pub mod embassy;

use core::convert::Infallible;
use core::fmt::{self, Display, Formatter};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use defmt_rtt as _;
use embassy_stm32::mode::Blocking;
use embassy_stm32::pac;
use embassy_stm32::pac::rcc::regs::Rsr;
use embassy_stm32::peripherals::{PD8, PD9, USART3};
use embassy_stm32::time::Hertz;
use embassy_stm32::usart::{self, ConfigError, Uart, UartRx, UartTx};
use embassy_stm32::{Config, Peri, Peripherals};
use embedded_io::{ErrorType, Read, Write, WriteReady};
use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};
use semihosting as _;

/// The CPU clock that `init` sets.
const CPU_MHZ: u32 = 600;

/// RCC's reset flags as `init` found them, which `info` names (B-12).
static RESET_FLAGS: AtomicU32 = AtomicU32::new(0);

/// Whether `reset` made the last reset, as `init` found the marker.
static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// Brings up the chip at 600 MHz from the board's 24 MHz crystal and returns every peripheral. The application passes
/// on the ones the board crate's other functions take, and keeps the rest.
///
/// It also notes why the chip last reset, for `boot_count` and `info`, and clears RCC's reset flags and the marker
/// that `reset` leaves, so the next boot sees only its own reset. Call it once per boot.
pub fn init() -> Peripherals {
    let flags = pac::RCC.rsr().read();
    RESET_FLAGS.store(flags.0, Ordering::Relaxed);
    pac::RCC.rsr().modify(|w| w.set_rmvf(true));
    pac::RCC.rsr().modify(|w| w.set_rmvf(false));
    // After a power-on or a brown-out the RAM has never been written, so the marker is not read, in case the RAM's
    // ECC flags a word that was never written. A watchdog between reset's marker write and its reset request leaves
    // the marker without the software flag, so both are needed.
    let cold = flags.porrstf() || flags.borrstf();
    critical_section::with(|_| {
        // SAFETY: only init, boot_count and reset touch PERSISTENT, each inside a critical section that masks
        // interrupts on this single core, so this is the only access.
        #[expect(unsafe_code)]
        unsafe {
            let marker = &raw mut PERSISTENT.marker;
            let software = !cold && flags.sftrstf() && marker.read_volatile() == SOFTWARE_RESET;
            SOFTWARE.store(software, Ordering::Relaxed);
            marker.write_volatile(0);
        }
    });

    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hse = Some(Hse {
            freq: Hertz(24_000_000),
            mode: HseMode::Oscillator,
        });
        // 24 MHz / 3 * 150 / 2 = 600 MHz.
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV3,
            mul: PllMul::MUL150,
            divp: Some(PllDiv::DIV2),
            divq: None,
            divr: None,
            divs: None,
            divt: None,
        });
        config.rcc.sys = Sysclk::PLL1_P;
        // HCLK at 300 MHz also clocks XSPI2, which the start-up code divides by 8 for the flash.
        config.rcc.ahb_pre = AHBPrescaler::DIV2;
        config.rcc.apb1_pre = APBPrescaler::DIV2;
        config.rcc.apb2_pre = APBPrescaler::DIV2;
        config.rcc.apb4_pre = APBPrescaler::DIV2;
        config.rcc.apb5_pre = APBPrescaler::DIV2;
        config.rcc.voltage_scale = VoltageScale::HIGH;
        // The start-up code has already written this, the only supply configuration a boot may write (boot.rs).
        config.rcc.supply_config = SupplyConfig::LDO;
    }
    embassy_stm32::init(config)
}

/// What survives a reset: the boot count, and the marker that `reset` leaves for the next boot. Its address follows
/// `.bss`, so it moves between firmware images, and the values mean something only to the image that wrote them.
/// Flashing another image is a probe's reset, which restarts the count anyway.
struct Persistent {
    count: u32,
    marker: u32,
}

/// `reset` writes this to the marker. RCC cannot tell a software reset from a probe's reset, because both are a
/// system reset request, so the marker is what makes a reset count as software.
const SOFTWARE_RESET: u32 = 0x50F7_8E5E;

// SAFETY: cortex-m-rt's link.x defines .uninit as a NOLOAD section in RAM, which start-up leaves alone. The RAM is
// DTCM, which keeps its contents across a system reset.
#[allow(unsafe_code)]
#[unsafe(link_section = ".uninit.PERSISTENT")]
static mut PERSISTENT: Persistent = Persistent {
    count: 0,
    marker: 0,
};

/// Counts this boot and returns the count. The count restarts at 1 after any reset that `reset` did not make,
/// because the RAM it lives in holds whatever it held before. Call it once per boot, after `init` and before the
/// console starts.
pub fn boot_count() -> u32 {
    let software = SOFTWARE.load(Ordering::Relaxed);
    critical_section::with(|_| {
        // SAFETY: only init, boot_count and reset touch PERSISTENT, each inside a critical section that masks
        // interrupts on this single core, so this is the only access. Volatile access stops the compiler from
        // assuming the values it was declared with, since start-up does not write them. No safe API in
        // embassy-stm32 reaches persistent storage on this chip.
        #[expect(unsafe_code)]
        unsafe {
            let count = &raw mut PERSISTENT.count;
            let next = if software {
                count.read_volatile().wrapping_add(1)
            } else {
                1
            };
            count.write_volatile(next);
            next
        }
    })
}

/// The name B-12 gives a reset, from RCC's reset flags and whether `reset` made it: `power-on`, `brown-out`,
/// `watchdog`, `software`, `debugger`, `pin`, or `other` followed by RCC's name for the flag in brackets. It is
/// plain `other` when no flag is set.
///
/// The chip drives NRST low during every reset, so the pin flag comes with every other flag, and `pin` is the name
/// only when no other flag says more. A probe's reset is a system reset request, as `reset` makes, but without the
/// marker that `reset` leaves, so it is `debugger`.
pub fn reset_name(flags: Rsr, software: bool) -> &'static str {
    if flags.porrstf() {
        "power-on"
    } else if flags.borrstf() {
        "brown-out"
    } else if flags.iwdgrstf() || flags.wwdgrstf() {
        "watchdog"
    } else if flags.sftrstf() {
        if software { "software" } else { "debugger" }
    } else if flags.lpwrrstf() {
        "other (LPWRRSTF)"
    } else if flags.oblrstf() {
        "other (OBLRSTF)"
    } else if flags.pinrstf() {
        "pin"
    } else {
        "other"
    }
}

/// The `info` command (B-10): eight `key: value` lines that describe the board and the application.
pub struct Info {
    firmware: &'static str,
}

static INFO_HELP: [Help; 1] = [Help {
    usage: "info",
    summary: "describe the board",
    secret: false,
}];

/// The `info` command for `firmware`, its name and version, such as `my-app 0.1.0`.
pub fn info(firmware: &'static str) -> Info {
    Info { firmware }
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
        let board = env!("CARGO_PKG_NAME");
        reply
            .field(
                "board",
                board.strip_prefix("embedded-devkit-").unwrap_or(board),
            )
            .await;
        reply
            .field("chip", Chip(pac::DBGMCU.idc().read().rev_id()))
            .await;
        reply.field("id", Id(embassy_stm32::uid::uid())).await;
        reply.field("flash", "64 KB + 32 MB external").await;
        reply.field("psram", "none").await;
        reply.field("cpu", format_args!("{CPU_MHZ} MHz")).await;
        let flags = Rsr(RESET_FLAGS.load(Ordering::Relaxed));
        reply
            .field("reset", reset_name(flags, SOFTWARE.load(Ordering::Relaxed)))
            .await;
        reply.field("firmware", self.firmware).await;
        CommandResult::Ok
    }
}

/// The chip and its revision, from DBGMCU's revision ID. RM0477 names revision ID `0x2000` B, the revision of the
/// boards in `REQUIREMENTS.md`. Another revision ID is written in hex.
struct Chip(u16);

impl Display for Chip {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            0x2000 => f.write_str("STM32H7S3L8 rev B"),
            other => write!(f, "STM32H7S3L8 rev {other:#06x}"),
        }
    }
}

/// The 96-bit unique device ID as 24 lowercase hex digits, in memory order (B-11).
struct Id([u8; 12]);

impl Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// Resets the chip from software. The next boot counts it as a software reset.
pub fn reset() -> ! {
    critical_section::with(|_| {
        // SAFETY: as in boot_count.
        #[expect(unsafe_code)]
        unsafe {
            (&raw mut PERSISTENT.marker).write_volatile(SOFTWARE_RESET);
        }
    });
    cortex_m::peripheral::SCB::sys_reset()
}

/// The GNU build ID of the running image, for the console's [`Config::image`](mcu_console::Config::image), or
/// `None` when the note in flash holds none. The firmware must link this crate's `mcu-run.x` with
/// `--build-id=sha1`, which places the note at `__mcu_build_id` in internal flash (B-24).
pub fn image_id() -> Option<&'static [u8]> {
    #[expect(unsafe_code)]
    unsafe extern "C" {
        static __mcu_build_id: [u8; 36];
    }
    // SAFETY: `mcu-run.x` puts `__mcu_build_id` at the start of the build-ID note, and its ASSERT fails the link
    // when the note is not 36 bytes. The note is in internal flash, which nothing writes while the firmware runs.
    #[expect(unsafe_code)]
    let note = unsafe { &__mcu_build_id };
    mcu_console::gnu_build_id(note)
}

/// The console's transport: USART3 at 115200 bps on PD9 (RX) and PD8 (TX), the board's virtual COM port.
///
/// # Errors
///
/// Returns embassy-stm32's `ConfigError` when USART3's clock cannot make 115200 bps.
pub fn console(
    usart: Peri<'static, USART3>,
    rx: Peri<'static, PD9>,
    tx: Peri<'static, PD8>,
) -> Result<(ConsoleRx, ConsoleTx), ConfigError> {
    let (tx, rx) = Uart::new_blocking(usart, rx, tx, usart::Config::default())?.split();
    Ok((ConsoleRx(rx), ConsoleTx(tx)))
}

/// The console's receiver, which reads the UART one byte at a time.
pub struct ConsoleRx(UartRx<'static, Blocking>);

/// The console's transmitter. It is ready when the UART's 16-byte transmit FIFO has room.
pub struct ConsoleTx(UartTx<'static, Blocking>);

impl ErrorType for ConsoleRx {
    type Error = Infallible;
}

impl Read for ConsoleRx {
    /// Waits for one byte. A framing, noise, parity or overrun error loses that byte, and the read waits for the
    /// next, because an error from `read` stops the console.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
        let Some(byte) = buf.first_mut() else {
            return Ok(0);
        };
        while self.0.blocking_read(core::slice::from_mut(byte)).is_err() {}
        Ok(1)
    }
}

impl ErrorType for ConsoleTx {
    type Error = usart::Error;
}

impl Write for ConsoleTx {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.blocking_write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.blocking_flush()
    }
}

impl WriteReady for ConsoleTx {
    fn write_ready(&mut self) -> Result<bool, Self::Error> {
        Ok(pac::USART3.isr().read().txe())
    }
}
