//! `perf run` over a fake platform and embassy-time's mock clock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use embassy_time::{Duration, MockDriver};
use mcu_console::{Args, Cancel, CancelSignal, CommandResult, Commands, Help, Reply};
use mcu_console_perf::{Heap, Perf, Platform, Stack};
use std::cell::RefCell;
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

/// The mock clock is one for the process, so the tests take turns.
static CLOCK: Mutex<()> = Mutex::new(());

/// The clock to this test, also after another test panicked while holding it.
fn clock() -> std::sync::MutexGuard<'static, ()> {
    CLOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the fake board counts. The test's `work` command moves it on.
#[derive(Default)]
struct Board {
    counter: u64,
    instructions: Option<u64>,
    stack: Option<Stack>,
    heap: Option<Heap>,
    repaints: usize,
}

/// A platform over a shared [`Board`], whose counter is `W` bits wide and counts at `HZ`.
struct Fake<const W: u32, const HZ: u32>(Rc<RefCell<Board>>);

impl<const W: u32, const HZ: u32> Platform for Fake<W, HZ> {
    const WIDTH: u32 = W;

    fn now(&self) -> u64 {
        let counter = self.0.borrow().counter;
        if W >= 64 {
            counter
        } else {
            counter & ((1 << W) - 1)
        }
    }

    fn hz(&self) -> u32 {
        HZ
    }

    fn instructions(&self) -> Option<u64> {
        self.0.borrow().instructions
    }

    fn repaint_stack(&self) {
        self.0.borrow_mut().repaints += 1;
    }

    fn stack(&self) -> Option<Stack> {
        self.0.borrow().stack
    }

    fn heap(&self) -> Option<Heap> {
        self.0.borrow().heap
    }
}

/// The application's commands: `work <us> <counts> <instructions>` moves the clock and the board on and writes
/// `worked`, `lock <counts>...` records one held lock for each count, `fail` fails, and `wait` returns `stopped`
/// once asked to stop.
struct App(Rc<RefCell<Board>>);

static APP_HELP: [Help; 1] = [Help {
    usage: "work <us> <counts> <instructions>",
    summary: "pass time",
    secret: false,
}];

impl Commands for App {
    fn help(&self) -> impl Iterator<Item = &'static Help> {
        APP_HELP.iter()
    }

    async fn run<R: Reply>(
        &mut self,
        name: &str,
        mut args: Args<'_>,
        reply: &mut R,
        cancel: Cancel<'_>,
    ) -> CommandResult {
        match name {
            "work" => {
                let mut number = || args.next().unwrap().parse::<u64>().unwrap();
                let (us, counts, instructions) = (number(), number(), number());
                MockDriver::get().advance(Duration::from_micros(us));
                {
                    let mut board = self.0.borrow_mut();
                    board.counter += counts;
                    if let Some(retired) = board.instructions.as_mut() {
                        *retired += instructions;
                    }
                    if let Some(stack) = board.stack.as_mut() {
                        stack.deepest = 1024;
                    }
                }
                reply.line("worked").await;
                CommandResult::Ok
            }
            "lock" => {
                for lock in args {
                    let (counts, caller) = match lock.split_once('@') {
                        Some((counts, caller)) => (
                            counts,
                            Some(
                                usize::from_str_radix(caller.trim_start_matches("0x"), 16).unwrap(),
                            ),
                        ),
                        None => (lock, None),
                    };
                    mcu_console_perf::lock_held(counts.parse().unwrap(), caller);
                }
                CommandResult::Ok
            }
            "fail" => CommandResult::Err("broken"),
            "wait" => {
                cancel.requested().await;
                reply.line("stopped").await;
                CommandResult::Ok
            }
            _ => CommandResult::Unknown,
        }
    }
}

#[derive(Default)]
struct Lines(Vec<String>);

impl Reply for Lines {
    async fn line(&mut self, text: &str) {
        self.0.push(text.to_owned());
    }
    async fn flush(&mut self) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..1000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("the command did not finish");
}

/// Runs `line` through `commands`, with `cancel` raised first when `stop` is set.
fn send(commands: &mut impl Commands, line: &str, stop: bool) -> (Vec<String>, CommandResult) {
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    let mut lines = Lines::default();
    let signal = CancelSignal::new();
    if stop {
        signal.raise();
    }
    let result = block_on(commands.run(name, Args::new(rest), &mut lines, signal.cancel()));
    (lines.0, result)
}

fn board() -> Rc<RefCell<Board>> {
    Rc::new(RefCell::new(Board {
        counter: 7,
        instructions: Some(11),
        stack: Some(Stack {
            size: 4096,
            deepest: 0,
        }),
        heap: Some(Heap {
            size: 1024,
            used: 100,
            peak: 300,
        }),
        repaints: 0,
    }))
}

#[test]
fn perf_run_writes_the_command_s_lines_then_its_figures() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 320_000_000>(board.clone()), App(board.clone()));
    let (lines, result) = send(&mut perf, "perf run work 1000 320000 100000", false);
    assert_eq!(result, CommandResult::Ok);
    assert_eq!(
        lines,
        [
            "worked",
            "perf_us: 1000",
            "perf_cycles: 320000",
            "perf_cpi: 3.20",
            "perf_stack: size 4096 deepest 1024 headroom 3072",
            "perf_heap: used 100 peak 300 free 924",
            "perf_locks: none",
        ]
    );
    assert_eq!(board.borrow().repaints, 1);
}

#[test]
fn perf_locks_gives_the_longest_the_count_and_the_mean_of_the_locks_in_the_window() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 320_000_000>(board.clone()), App(board));
    // A lock before the window is not counted.
    mcu_console_perf::lock_held(320_000, Some(0x42ff_fff0));
    let (lines, _) = send(
        &mut perf,
        "perf run lock 320@0x42000010 640@0x4200abcd 32@0x42000020",
        false,
    );
    assert_eq!(
        lines.last().unwrap(),
        "perf_locks: longest_us 2.0 count 3 mean_us 1.0 at 0x4200abcd"
    );
    let (lines, _) = send(&mut perf, "perf run fail", false);
    assert_eq!(lines.last().unwrap(), "perf_locks: none");
}

#[test]
fn perf_locks_gives_none_for_the_caller_when_the_longest_lock_has_none() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 320_000_000>(board.clone()), App(board));
    let (lines, _) = send(&mut perf, "perf run lock 640@0x4200abcd 960 320", false);
    assert_eq!(
        lines.last().unwrap(),
        "perf_locks: longest_us 3.0 count 3 mean_us 2.0 at none"
    );
}

#[test]
fn perf_locks_keeps_the_first_caller_of_two_locks_of_the_longest_length() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 320_000_000>(board.clone()), App(board));
    let (lines, _) = send(
        &mut perf,
        "perf run lock 640@0x4200abcd 640@0x4200beef",
        false,
    );
    assert_eq!(
        lines.last().unwrap(),
        "perf_locks: longest_us 2.0 count 2 mean_us 2.0 at 0x4200abcd"
    );
}

#[test]
fn a_figure_the_board_does_not_give_is_none() {
    let _clock = clock();
    let board = Rc::new(RefCell::new(Board::default()));
    let mut perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board));
    let (lines, _) = send(&mut perf, "perf run work 10 10 0", false);
    assert_eq!(
        lines[3..],
        [
            "perf_cpi: none",
            "perf_stack: none",
            "perf_heap: none",
            "perf_locks: none"
        ]
    );
}

#[test]
fn a_narrow_counter_that_wraps_in_the_window_still_gives_the_exact_count() {
    let _clock = clock();
    let board = board();
    // A 24-bit counter at 200 MHz wraps every 84 ms. The command runs 100 ms, and the counter runs 5% fast, less
    // than half a wrap off the clock.
    let mut perf = Perf::new(Fake::<24, 200_000_000>(board.clone()), App(board));
    let (lines, _) = send(&mut perf, "perf run work 100000 21000000 0", false);
    assert_eq!(lines[2], "perf_cycles: 21000000");
}

#[test]
fn the_command_s_result_is_perf_run_s_result() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board));
    let (lines, result) = send(&mut perf, "perf run fail", false);
    assert_eq!(result, CommandResult::Err("broken"));
    assert_eq!(lines[0], "perf_us: 0");
}

#[test]
fn a_stop_reaches_the_command_that_perf_run_runs() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board));
    let (lines, result) = send(&mut perf, "perf run wait", true);
    assert_eq!(result, CommandResult::Ok);
    assert_eq!(lines[0], "stopped");
}

#[test]
fn a_line_without_perf_goes_to_the_application_unchanged() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board.clone()));
    assert_eq!(
        send(&mut perf, "work 1 1 1", false),
        (vec!["worked".to_owned()], CommandResult::Ok)
    );
    assert_eq!(board.borrow().repaints, 0);
    assert_eq!(send(&mut perf, "frob", false).1, CommandResult::Unknown);
}

#[test]
fn a_bad_perf_line_fails_with_the_usage_or_the_unknown_command() {
    let _clock = clock();
    let board = board();
    let mut perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board));
    let usage = CommandResult::Err("usage: perf run <command> [args]");
    assert_eq!(send(&mut perf, "perf", false), (vec![], usage));
    assert_eq!(send(&mut perf, "perf frob", false), (vec![], usage));
    assert_eq!(send(&mut perf, "perf run", false), (vec![], usage));
    assert_eq!(
        send(&mut perf, "perf run frob", false),
        (vec![], CommandResult::Err("perf run: unknown command"))
    );
}

#[test]
fn help_lists_perf_run_then_the_application_s_commands() {
    let board = board();
    let perf = Perf::new(Fake::<64, 1_000_000>(board.clone()), App(board));
    let usages: Vec<&str> = perf.help().map(|help| help.usage).collect();
    assert_eq!(
        usages,
        [
            "perf run <command> [args]",
            "work <us> <counts> <instructions>"
        ]
    );
}
