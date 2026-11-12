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

use embedded_devkit_nucleo_h7s3l8 as _;

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use embassy_time::{Duration, Instant};
    use embedded_devkit_nucleo_h7s3l8::{boot_count, console, init};
    use embedded_io::{Write, WriteReady};

    // The start-up code, which cortex-m-rt's Reset calls. Only its address is taken.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn __pre_init();
    }

    /// B-14. The test itself runs, so the start-up code mapped the external flash. The code must also be where the
    /// layout puts it: the crate's functions in external flash, the start-up code in internal flash.
    #[test]
    fn the_code_runs_from_external_flash_and_starts_from_internal_flash() {
        let external = 0x7000_0000..0x7200_0000;
        let internal = 0x0800_0000..0x0801_0000;
        assert!(external.contains(&(init as *const () as usize)));
        assert!(internal.contains(&(__pre_init as *const () as usize)));
    }

    #[test]
    fn a_boot_after_a_probe_reset_counts_one() {
        let _peripherals = init();
        assert_eq!(boot_count(), 1);
    }

    /// The UART sends a byte in about 87 µs at 115200 bps, from a 16-byte FIFO. Thirty-two bytes written back to back
    /// leave the FIFO full, so the transmitter is not ready until the next byte has gone.
    #[test]
    fn the_transmitter_is_ready_exactly_when_its_fifo_has_room() {
        let peripherals = init();
        let (_rx, mut tx) = console(peripherals.USART3, peripherals.PD9, peripherals.PD8).unwrap();
        assert!(tx.write_ready().unwrap());
        assert_eq!(tx.write(&[b'x'; 32]), Ok(32));
        assert!(!tx.write_ready().unwrap());
        let start = Instant::now();
        while !tx.write_ready().unwrap() {
            assert!(start.elapsed() < Duration::from_millis(1));
        }
    }

    /// B-12. The chip drives NRST low during every reset, so the pin flag comes with every other flag, and the
    /// pin is the reason only when no other flag says more. A probe's reset is a system reset request without the
    /// marker that `reset` leaves.
    #[test]
    fn every_reset_flag_maps_to_its_name() {
        use embassy_stm32::pac::rcc::regs::Rsr;
        use embedded_devkit_nucleo_h7s3l8::reset_name;
        let flags = |set: fn(&mut Rsr)| {
            let mut flags = Rsr(0);
            flags.set_pinrstf(true);
            set(&mut flags);
            flags
        };
        let names = [
            (
                flags(|r| {
                    r.set_porrstf(true);
                    r.set_borrstf(true);
                }),
                false,
                "power-on",
            ),
            (flags(|r| r.set_borrstf(true)), false, "brown-out"),
            (flags(|r| r.set_iwdgrstf(true)), false, "watchdog"),
            (flags(|r| r.set_wwdgrstf(true)), false, "watchdog"),
            (flags(|r| r.set_sftrstf(true)), true, "software"),
            (flags(|r| r.set_sftrstf(true)), false, "debugger"),
            (flags(|_| {}), false, "pin"),
            (flags(|r| r.set_lpwrrstf(true)), false, "other (LPWRRSTF)"),
            (flags(|r| r.set_oblrstf(true)), false, "other (OBLRSTF)"),
            (Rsr(0), false, "other"),
        ];
        for (flags, marked, name) in names {
            assert_eq!(reset_name(flags, marked), name);
        }
    }

    #[test]
    #[should_panic]
    fn a_panic_is_expected_here() {
        panic!("expected");
    }
}
