//! The board crate's on-target tests. `mcu-run` resets the chip through the probe before each test, so each one
//! starts from a boot that is not a software reset. One test panics on purpose, so `mcu-run` shows that it
//! reports an expected panic as a pass (M-11).

#![no_std]
#![no_main]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use embedded_devkit_esp32_c5_devkitc_1 as _;

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use embedded_devkit_esp32_c5_devkitc_1::{boot_count, console, init};
    use embedded_io::{ErrorKind, Write, WriteReady};
    use esp_hal::time::{Duration, Instant};

    #[test]
    fn a_boot_after_a_probe_reset_counts_one() {
        let _peripherals = init();
        assert_eq!(boot_count(), 1);
    }

    /// Nothing on the host reads the console during a test, and the ROM's boot output has already filled the FIFO.
    /// A transmitter that waits for the host would never finish.
    #[test]
    fn the_transmitter_takes_a_byte_exactly_when_ready_and_never_waits() {
        let peripherals = init();
        let (_rx, mut tx) = console(
            peripherals.USB_DEVICE,
            peripherals.GPIO14,
            peripherals.GPIO13,
        )
        .unwrap();
        let start = Instant::now();
        for _ in 0..1000 {
            if tx.write_ready().unwrap() {
                assert_eq!(tx.write(b"x"), Ok(1));
            } else {
                assert_eq!(tx.write(b"x"), Err(ErrorKind::WriteZero));
            }
            tx.flush().unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    /// B-12. The C5 has no reason for the reset pin, because the EN pin resets the chip as a power-on does. A
    /// debug probe's reset is `Cpu0JtagCpu`, and every reason B-12 does not name has the chip's own name.
    #[test]
    fn every_c5_reset_reason_maps_to_its_name() {
        use embedded_devkit_esp32_c5_devkitc_1::reset_name;
        use esp_hal::rtc_cntl::SocResetReason::*;
        let names = [
            (ChipPowerOn, "power-on"),
            (CoreSw, "software"),
            (Cpu0Sw, "software"),
            (CoreDeepSleep, "other (CoreDeepSleep)"),
            (CoreMwdt0, "watchdog"),
            (CoreMwdt1, "watchdog"),
            (CoreRtcWdt, "watchdog"),
            (Cpu0Mwdt0, "watchdog"),
            (Cpu0RtcWdt, "watchdog"),
            (SysBrownOut, "brown-out"),
            (SysRtcWdt, "watchdog"),
            (Cpu0Mwdt1, "watchdog"),
            (SysSuperWdt, "watchdog"),
            (CoreEfuseCrc, "other (CoreEfuseCrc)"),
            (CoreUsbUart, "other (CoreUsbUart)"),
            (CoreUsbJtag, "debugger"),
            (Cpu0JtagCpu, "debugger"),
            (PowerGlitch, "other (PowerGlitch)"),
            (CpuLockup, "other (CpuLockup)"),
        ];
        for (reason, name) in names {
            assert_eq!(reset_name(Some(reason)), name);
        }
        assert_eq!(reset_name(None), "other");
    }

    #[test]
    #[should_panic]
    fn a_panic_is_expected_here() {
        panic!("expected");
    }
}
