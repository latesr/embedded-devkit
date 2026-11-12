//! Performance figures for `mcu-console`.
//!
//! [`Perf`] is a command set that wraps the application's own. `perf run <command> [args]` runs one of the
//! application's commands, writes that command's lines, and then writes the figures for the time it ran:
//!
//! - `perf_us: <n>`, the time it took, from embassy-time
//! - `perf_cycles: <n>`, the counts of the board's counter, usually CPU cycles
//! - `perf_cpi: <n.nn>`, cycles per instruction retired. It counts everything the core ran in the window, such
//!   as interrupts and other threads, not only the command. On a core whose cycle counter runs while it waits for
//!   an interrupt, as the ESP32-S31's does, a command that mostly waits has a large figure. Over a busy window, a
//!   rise means stalls, such as cache misses.
//! - `perf_stack: size <b> deepest <b> headroom <b>`, the main stack's deepest point during the command
//! - `perf_heap: used <b> peak <b> free <b>`
//! - `perf_locks: longest_us <n.n> count <n> mean_us <n.n> at <0xaddr>`, the locks that the board's timed mutex
//!   recorded through [`lock_held`]. `at` gives the address of the code that took the longest lock, or `none`.
//!   Other locks, such as the HAL's own critical sections, are not in it.
//!
//! A figure the board does not give is written as `none`. The result line is the command's own. A line that does
//! not start with `perf` goes to the application's commands unchanged.
//!
//! The board gives the figures through [`Platform`]. Only a counter and its rate are required, so a part with
//! only a 24-bit SysTick meets it. A board crate gives its `Platform` behind a `perf` feature, and only the
//! application turns it on. Cargo turns a feature on for the whole build, so a library that turned it on would
//! give every application its cost.

#![cfg_attr(not(test), no_std)]
// The commands run on one executor, as mcu-console's do.
#![allow(async_fn_in_trait)]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use embassy_time::Instant;
use mcu_console::{Args, Cancel, CommandResult, Commands, Help, Reply};

/// What a board gives `mcu-console-perf`.
pub trait Platform {
    /// The bits in the counter that [`now`](Self::now) reads. A counter narrower than 64 bits wraps, and
    /// `mcu-console-perf` counts the wraps from the window's time, so its count stays exact while the board's clock is
    /// off by less than half a wrap.
    const WIDTH: u32 = 64;

    /// The counter, such as the CPU cycle counter. Only its low [`WIDTH`](Self::WIDTH) bits are used.
    fn now(&self) -> u64;

    /// How many times a second the counter counts.
    fn hz(&self) -> u32;

    /// The instructions retired, or `None` when the core does not count them. The count must not wrap within a
    /// window.
    fn instructions(&self) -> Option<u64> {
        None
    }

    /// Marks the free part of the main stack, so [`stack`](Self::stack) reports the deepest point from now on.
    fn repaint_stack(&self) {}

    /// The main stack's size and its deepest point since the last [`repaint_stack`](Self::repaint_stack), or
    /// `None`.
    fn stack(&self) -> Option<Stack> {
        None
    }

    /// The heap, or `None` for an application without one.
    fn heap(&self) -> Option<Heap> {
        None
    }
}

/// The main stack, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stack {
    /// The stack's size.
    pub size: usize,
    /// The most of it that was in use.
    pub deepest: usize,
}

/// The heap, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heap {
    /// The heap's size.
    pub size: usize,
    /// What is allocated now.
    pub used: usize,
    /// The most that was allocated at once.
    pub peak: usize,
}

/// Records one lock that a board's timed mutex held for `counts` of the [`Platform`] counter, for the window
/// that is open. `caller` is the address of the code that took the lock, or `None` when the board cannot tell.
/// The window keeps the caller of its longest lock. A lock taken inside another timed lock counts in both. The
/// window's total held time wraps after 2^32 counts.
///
/// It uses only atomics, so an interrupt may call it. An interrupt that records a longer lock between the new
/// longest time and its caller can leave the longest time with the other lock's caller. On a core without
/// compare-and-swap, such as a Cortex-M0, it reads and then writes each figure, and an interrupt that records a
/// lock in between can lose one of the two updates.
pub fn lock_held(counts: u32, caller: Option<usize>) {
    #[cfg(target_has_atomic = "32")]
    {
        if counts > LOCKS.longest.fetch_max(counts, Ordering::Relaxed) {
            LOCKS.caller.store(caller.unwrap_or(0), Ordering::Relaxed);
        }
        LOCKS.count.fetch_add(1, Ordering::Relaxed);
        LOCKS.total.fetch_add(counts, Ordering::Relaxed);
    }
    #[cfg(not(target_has_atomic = "32"))]
    lock_held_without_cas(counts, caller);
}

#[cfg(not(target_has_atomic = "32"))]
fn lock_held_without_cas(counts: u32, caller: Option<usize>) {
    if counts > LOCKS.longest.load(Ordering::Relaxed) {
        LOCKS.longest.store(counts, Ordering::Relaxed);
        LOCKS.caller.store(caller.unwrap_or(0), Ordering::Relaxed);
    }
    let count = LOCKS.count.load(Ordering::Relaxed);
    LOCKS.count.store(count.wrapping_add(1), Ordering::Relaxed);
    let total = LOCKS.total.load(Ordering::Relaxed);
    LOCKS
        .total
        .store(total.wrapping_add(counts), Ordering::Relaxed);
}

/// The locks that [`lock_held`] recorded since [`Locks::reset`].
struct Locks {
    longest: AtomicU32,
    /// The caller of the longest lock, 0 when it is not known.
    caller: AtomicUsize,
    count: AtomicU32,
    total: AtomicU32,
}

impl Locks {
    fn reset(&self) {
        self.longest.store(0, Ordering::Relaxed);
        self.caller.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }
}

static LOCKS: Locks = Locks {
    longest: AtomicU32::new(0),
    caller: AtomicUsize::new(0),
    count: AtomicU32::new(0),
    total: AtomicU32::new(0),
};

/// `counts` of a counter at `hz`, in microseconds to a tenth.
struct Micros {
    counts: u64,
    hz: u32,
}

impl core::fmt::Display for Micros {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let tenths = self
            .counts
            .saturating_mul(10_000_000)
            .checked_div(u64::from(self.hz))
            .unwrap_or(0);
        write!(f, "{}.{}", tenths / 10, tenths % 10)
    }
}

/// The `perf` commands around the application's command set `C`.
pub struct Perf<P, C> {
    platform: P,
    commands: C,
}

impl<P: Platform, C: Commands> Perf<P, C> {
    /// `perf` over `platform`, running `commands`.
    pub fn new(platform: P, commands: C) -> Self {
        Self { platform, commands }
    }
}

const USAGE: &str = "usage: perf run <command> [args]";

static HELP: [Help; 1] = [Help {
    usage: "perf run <command> [args]",
    summary: "run a command, then write its time, cycles, stack and heap",
    secret: false,
}];

impl<P: Platform, C: Commands> Commands for Perf<P, C> {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        HELP.iter().chain(self.commands.help())
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        mut args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        if name != "perf" {
            return self.commands.run(name, args, reply, cancel).await;
        }
        if args.next() != Some("run") {
            return CommandResult::Err(USAGE);
        }
        let Some(inner) = args.next() else {
            return CommandResult::Err(USAGE);
        };
        // The paint runs before the window opens, so its time is not counted.
        self.platform.repaint_stack();
        LOCKS.reset();
        let start = Instant::now();
        let counts = self.platform.now();
        let retired = self.platform.instructions();
        let result = self.commands.run(inner, args, reply, cancel).await;
        if result == CommandResult::Unknown {
            return CommandResult::Err("perf run: unknown command");
        }
        let end_counts = self.platform.now();
        let end_retired = self.platform.instructions();
        let us = start.elapsed().as_micros();
        let (longest, caller, locks, held) = (
            LOCKS.longest.load(Ordering::Relaxed),
            LOCKS.caller.load(Ordering::Relaxed),
            LOCKS.count.load(Ordering::Relaxed),
            LOCKS.total.load(Ordering::Relaxed),
        );
        let expected = us.saturating_mul(u64::from(self.platform.hz())) / 1_000_000;
        let cycles = elapsed_counts(counts, end_counts, P::WIDTH, expected);
        reply.field("perf_us", us).await;
        reply.field("perf_cycles", cycles).await;
        let hundredths = retired.zip(end_retired).and_then(|(before, after)| {
            cycles
                .saturating_mul(100)
                .checked_div(after.wrapping_sub(before))
        });
        match hundredths {
            Some(hundredths) => {
                let (whole, part) = (hundredths / 100, hundredths % 100);
                reply
                    .field("perf_cpi", format_args!("{whole}.{part:02}"))
                    .await;
            }
            _ => reply.field("perf_cpi", "none").await,
        }
        match self.platform.stack() {
            Some(stack) => {
                let headroom = stack.size.saturating_sub(stack.deepest);
                let line = format_args!(
                    "size {} deepest {} headroom {headroom}",
                    stack.size, stack.deepest
                );
                reply.field("perf_stack", line).await;
            }
            None => reply.field("perf_stack", "none").await,
        }
        match self.platform.heap() {
            Some(heap) => {
                let free = heap.size.saturating_sub(heap.used);
                let line = format_args!("used {} peak {} free {free}", heap.used, heap.peak);
                reply.field("perf_heap", line).await;
            }
            None => reply.field("perf_heap", "none").await,
        }
        let hz = self.platform.hz();
        match u64::from(held).checked_div(u64::from(locks)) {
            Some(mean) => {
                let longest = Micros {
                    counts: u64::from(longest),
                    hz,
                };
                let mean = Micros { counts: mean, hz };
                let line = format_args!("longest_us {longest} count {locks} mean_us {mean}");
                match caller {
                    0 => {
                        let line = format_args!("{line} at none");
                        reply.field("perf_locks", line).await;
                    }
                    caller => {
                        let line = format_args!("{line} at {caller:#x}");
                        reply.field("perf_locks", line).await;
                    }
                }
            }
            None => reply.field("perf_locks", "none").await,
        }
        result
    }
}

/// The counts from `start` to `end` of a counter `width` bits wide, when the window's time says about `expected`.
/// The low bits come from the counter, and the number of wraps is the one that comes nearest to `expected`.
fn elapsed_counts(start: u64, end: u64, width: u32, expected: u64) -> u64 {
    let Some(wrap) = 1u64.checked_shl(width) else {
        return end.wrapping_sub(start);
    };
    let low = end.wrapping_sub(start) & wrap.wrapping_sub(1);
    let wraps = expected
        .saturating_add(wrap / 2)
        .saturating_sub(low)
        .checked_div(wrap)
        .unwrap_or(0);
    wraps.saturating_mul(wrap).saturating_add(low)
}
