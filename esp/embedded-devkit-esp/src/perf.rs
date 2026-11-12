//! `mcu-console-perf`'s figures for the ESP32-S31: the CPU cycles and the instructions retired, from the core's own
//! counters, the main stack's deepest point, and in an application with Wi-Fi, the heap.

use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
use mcu_console_perf::{Heap, Platform, Stack};

/// The S31's [`Platform`].
pub struct EspPlatform;

impl Platform for EspPlatform {
    fn now(&self) -> u64 {
        riscv::register::mcycle::read64()
    }

    fn hz(&self) -> u32 {
        esp_hal::clock::cpu_clock().as_hz()
    }

    fn instructions(&self) -> Option<u64> {
        Some(riscv::register::minstret::read64())
    }

    fn repaint_stack(&self) {
        stack::paint();
    }

    fn stack(&self) -> Option<Stack> {
        Some(stack::measure())
    }

    fn heap(&self) -> Option<Heap> {
        // An application that links the allocator and gives it no memory has no heap.
        #[cfg(feature = "wifi")]
        {
            let stats = esp_alloc::HEAP.stats();
            (stats.size > 0).then_some(Heap {
                size: stats.size,
                used: stats.current_usage,
                peak: stats.max_usage,
            })
        }
        #[cfg(not(feature = "wifi"))]
        None
    }
}

/// A `RawMutex` that locks as embassy-sync's `CriticalSectionRawMutex` does, and records how long each lock was
/// held through [`mcu_console_perf::lock_held`], for `perf_locks` (P-7). The time runs from before the critical section
/// starts to after it ends, in the CPU cycles that [`EspPlatform`] counts. The caller is the return address of
/// `lock`. `addr2line -f -i` on that address minus 1 gives the call that took the lock and its inlined callers.
pub struct TimedRawMutex(CriticalSectionRawMutex);

// SAFETY: the inner `CriticalSectionRawMutex` keeps `RawMutex`'s contract, and `lock` only reads the cycle counter
// around it and `ra` before it. embassy-sync's trait is unsafe to implement, no HAL offers a timed lock, and Rust
// has no stable way to read a return address.
#[expect(unsafe_code)]
unsafe impl RawMutex for TimedRawMutex {
    const INIT: Self = Self(CriticalSectionRawMutex::new());

    // Not inlined, so `ra` holds the return address into the caller. It is read before any call can change it.
    #[inline(never)]
    fn lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let caller: usize;
        unsafe {
            core::arch::asm!("mv {}, ra", out(reg) caller, options(nomem, nostack, preserves_flags))
        };
        // The low 32 bits wrap after 13 s at 320 MHz, far longer than one lock.
        let start = riscv::register::mcycle::read() as u32;
        let result = self.0.lock(f);
        let end = riscv::register::mcycle::read() as u32;
        mcu_console_perf::lock_held(end.wrapping_sub(start), Some(caller));
        result
    }
}

/// The main stack, from esp-hal's linker symbols. It grows down from `_stack_start` to `_stack_end`, and
/// esp-hal's stack guard word sits just above `_stack_end`, under a watchpoint.
mod stack {
    use mcu_console_perf::Stack;

    /// What the paint writes to each free word. A word that still holds it was not used.
    const PAINT: u32 = 0xA5A5_A5A5;

    // SAFETY: esp-hal's linker script defines the three symbols for every chip. The code takes only their
    // addresses, never reads them through these declarations.
    #[expect(unsafe_code)]
    unsafe extern "C" {
        static _stack_start: u32;
        static _stack_end: u32;
        static __stack_chk_guard: u32;
    }

    /// The first word above the guard, and the top of the stack.
    fn bounds() -> (usize, usize) {
        let guard = &raw const __stack_chk_guard as usize;
        (guard.saturating_add(4), &raw const _stack_start as usize)
    }

    /// Writes [`PAINT`] to every word from above the guard up to the stack pointer.
    pub(super) fn paint() {
        let (bottom, _) = bounds();
        critical_section::with(|_| {
            let sp: usize;
            // SAFETY: the words from `bottom` up to `sp` are below every live frame, because the stack grows
            // down, and no interrupt can push a frame there while the critical section holds. The loop calls
            // nothing, so it uses no stack below `sp` itself. `bottom` is above esp-hal's guard word, so the
            // watchpoint on it does not fire. No HAL reports the stack's deepest point.
            #[expect(unsafe_code)]
            unsafe {
                core::arch::asm!("mv {}, sp", out(reg) sp, options(nomem, nostack));
                let mut word = bottom;
                while word < sp {
                    core::ptr::write_volatile(word as *mut u32, PAINT);
                    word = word.wrapping_add(4);
                }
            }
        });
    }

    /// The stack's size, and its deepest point since the last [`paint`]: the distance from the top to the lowest
    /// word that no longer holds [`PAINT`].
    pub(super) fn measure() -> Stack {
        let (bottom, top) = bounds();
        let size = top.saturating_sub(&raw const _stack_end as usize);
        let mut word = bottom;
        // SAFETY: every word from `bottom` up to `top` is stack RAM, and reading one has no side effect. A word that a frame writes while this reads is not `PAINT` either way.
        #[expect(unsafe_code)]
        while word < top && unsafe { core::ptr::read_volatile(word as *const u32) } == PAINT {
            word = word.wrapping_add(4);
        }
        Stack {
            size,
            deepest: top.saturating_sub(word),
        }
    }
}
