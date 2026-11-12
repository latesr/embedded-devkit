//! The interrupt executors that `interrupt_executors!` writes. Embassy issue #2603 asks whether interrupt
//! executors work on the STM32H7, so this test shows that the high one preempts a task that keeps the low one busy.

#![no_std]
#![no_main]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

embedded_devkit_nucleo_h7s3l8::interrupt_executors!();

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use embassy_time::{Duration, Instant, Timer, block_for};
    use embedded_devkit_nucleo_h7s3l8::init;

    static BUSY_FROM: AtomicU32 = AtomicU32::new(0);
    static BUSY_UNTIL: AtomicU32 = AtomicU32::new(0);
    static WOKE_AT: AtomicU32 = AtomicU32::new(0);
    static DONE: AtomicBool = AtomicBool::new(false);

    fn now() -> u32 {
        Instant::now().as_micros() as u32
    }

    #[embassy_executor::task]
    async fn wake_after_5_ms() {
        Timer::after_millis(5).await;
        WOKE_AT.store(now(), Ordering::SeqCst);
    }

    #[embassy_executor::task]
    async fn stay_busy_for_50_ms() {
        BUSY_FROM.store(now(), Ordering::SeqCst);
        block_for(Duration::from_millis(50));
        BUSY_UNTIL.store(now(), Ordering::SeqCst);
        DONE.store(true, Ordering::SeqCst);
    }

    /// The high task waits on a 5 ms timer, then the low task spins for 50 ms without awaiting. The high task's
    /// timer fires while the low task spins, so the high task runs inside the busy window only if its executor
    /// preempts the low one.
    #[test]
    fn the_high_executor_preempts_a_busy_task_on_the_low_one() {
        let _peripherals = init();
        let (high, low) = super::start_executors();
        high.spawn(wake_after_5_ms().unwrap());
        low.spawn(stay_busy_for_50_ms().unwrap());
        while !DONE.load(Ordering::SeqCst) {
            core::hint::spin_loop();
        }
        let (from, until, woke) = (
            BUSY_FROM.load(Ordering::SeqCst),
            BUSY_UNTIL.load(Ordering::SeqCst),
            WOKE_AT.load(Ordering::SeqCst),
        );
        defmt::info!("busy {} to {} us, high woke at {} us", from, until, woke);
        assert!(from < woke && woke < until);
        // On time, too: the timer fired close to its 5 ms, not just somewhere inside the 50 ms spin.
        assert!(woke < from + 10_000);
    }
}
