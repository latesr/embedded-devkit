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

use embedded_devkit_esp32_s31_function_coreboard_1 as _;

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use embedded_devkit_esp32_s31_function_coreboard_1::{boot_count, console, init, psram};
    use embedded_io::{ErrorKind, Write, WriteReady};
    use esp_hal::time::{Duration, Instant};

    #[test]
    fn a_boot_after_a_probe_reset_counts_one() {
        let _peripherals = init();
        assert_eq!(boot_count(), 1);
    }

    /// After `init`, the MPLL runs at twice the PSRAM frequency that `ESP_HAL_CONFIG_PSRAM_FREQUENCY` sets, read
    /// back from its dividers: the 40 MHz crystal times (`fb_div` + 1), divided by (`ref_div` + 1).
    #[test]
    fn the_mpll_runs_at_twice_the_psram_frequency() {
        let peripherals = init();
        let _psram = psram(peripherals.PSRAM);
        let dividers = esp_hal::peripherals::LP_AON_CLK_RST::regs()
            .mspi_div()
            .read();
        let fb_div = u32::from(dividers.mspi_fb_div().bits());
        let ref_div = u32::from(dividers.mspi_ref_div().bits());
        let psram_mhz = esp_hal::psram::PsramTimingParams::default().clock;
        assert_eq!(40 * (fb_div + 1) / (ref_div + 1), 2 * psram_mhz);
    }

    /// The S31's PSRAM has its own window, from `0x5000_0000`. The PSRAM MMU cannot map flash, so code that
    /// runs at an address there runs from PSRAM.
    #[test]
    fn code_and_read_only_data_run_from_psram() {
        static READ_ONLY: [u32; 2] = [0x5053_524D; 2];
        #[inline(never)]
        fn in_text() -> u32 {
            core::hint::black_box(READ_ONLY[0])
        }
        let in_window = |addr: usize| (0x5000_0000..0x5400_0000).contains(&addr);

        let _peripherals = init();
        assert!(in_window(in_text as *const () as usize));
        assert!(in_window(core::ptr::addr_of!(READ_ONLY) as usize));
        assert_eq!(in_text(), 0x5053_524D);
    }

    /// The module has 16 MB of PSRAM. The code runs from its first pages, and every word of the rest holds
    /// the value written to it, at the PSRAM frequency that the configuration sets. The caches are far smaller than the rest, so most reads
    /// come from the PSRAM itself.
    #[test]
    fn psram_holds_a_pattern_after_the_code_region() {
        let peripherals = init();
        let psram = psram(peripherals.PSRAM);
        assert_eq!(psram.size(), 16 * 1024 * 1024);
        let (start, len) = psram.raw_parts();
        assert!(len > 0);
        let words = start.cast::<u32>();
        let count = len / 4;
        let pattern = |index: usize| (index as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A;
        // SAFETY: `raw_parts` gives the mapped PSRAM, which nothing else in this test uses, and every index is
        // below `len / 4`.
        #[expect(unsafe_code)]
        let mismatches = unsafe {
            for index in 0..count {
                words.add(index).write_volatile(pattern(index));
            }
            (0..count)
                .filter(|&index| words.add(index).read_volatile() != pattern(index))
                .count()
        };
        assert_eq!(mismatches, 0);
    }

    /// Nothing on the host reads the console during a test, and the ROM's boot output has already filled the FIFO.
    /// A transmitter that waits for the host would never finish.
    #[test]
    fn the_transmitter_takes_a_byte_exactly_when_ready_and_never_waits() {
        let peripherals = init();
        let (_rx, mut tx) = console(peripherals.USB_DEVICE);
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

    /// B-12. esp-hal names no reason for the reset pin on the S31. Every reason B-12 does not name has the
    /// chip's own name.
    #[test]
    fn every_s31_reset_reason_maps_to_its_name() {
        use embedded_devkit_esp32_s31_function_coreboard_1::reset_name;
        use esp_hal::rtc_cntl::SocResetReason::*;
        let names = [
            (ChipPowerOn, "power-on"),
            (CoreSw, "software"),
            (CoreDeepSleep, "other (CoreDeepSleep)"),
            (CpuPmuPwrDown, "other (CpuPmuPwrDown)"),
            (CoreMwdt0, "watchdog"),
            (CoreMwdt1, "watchdog"),
            (CoreRwdt, "watchdog"),
            (CpuMwdt, "watchdog"),
            (CpuSw, "software"),
            (CpuRwdt, "watchdog"),
            (SysBrownOut, "brown-out"),
            (SysRwdt, "watchdog"),
            (SysSuperWdt, "watchdog"),
            (CorePwrGlitch, "other (CorePwrGlitch)"),
            (CoreEfuseCrc, "other (CoreEfuseCrc)"),
            (CoreUsbJtag, "debugger"),
            (CoreUsbUart, "other (CoreUsbUart)"),
            (CpuJtag, "debugger"),
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
