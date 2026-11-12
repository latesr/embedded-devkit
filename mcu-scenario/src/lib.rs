//! Scenario tests for firmware that runs an `mcu-console` console.
//!
//! A scenario test is a host test that drives running firmware through its console, as a person at a terminal
//! would. This crate gives each test:
//!
//! 1. the firmware's ELF, built by cargo in the firmware's own folder, once per test process
//! 2. the board that `MCU_DEVICE` names, or else the one board attached that the firmware's chip can run on
//! 3. a console on freshly started firmware, restarted through the console's `reset` when the board already runs
//!    it, and flashed otherwise
//! 4. the board for that test alone, so tests on one board do not need `--test-threads=1`
//!
//! ```no_run
//! use mcu_scenario::{CommandResult, Firmware};
//!
//! static CONSOLE: Firmware = Firmware::example("console");
//!
//! # fn main() -> Result<(), mcu_scenario::Error> {
//! let mut console = CONSOLE.console()?;
//! let reply = console.send("info")?;
//! assert_eq!(reply.result, CommandResult::Ok);
//! assert_eq!(reply.field("board"), Some("esp32-s31-function-coreboard-1"));
//! # Ok(())
//! # }
//! ```
//!
//! A test that needs a board is marked `#[ignore]`, so `cargo test` passes without one, and
//! `cargo test -- --ignored` runs it.

mod cargo;
mod console;
mod error;
mod host;

pub use console::Console;
pub use error::Error;
pub use host::{HostAddress, Medium, host_address};
pub use mcu_run::{Banner, CommandResult, Reply};

use cargo::Target;
use mcu_run::{Board, Image, Runner};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Firmware that a scenario test starts: an example or a binary that cargo builds, or an ELF built elsewhere.
///
/// The builders are `const`, so a test file can hold its firmware in a `static`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Firmware {
    source: Source,
    dir: &'static str,
    features: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Built(Target, &'static str),
    Elf(&'static str),
}

impl Firmware {
    /// The example `name`, built in the test package's folder until [`in_dir`](Self::in_dir) names another.
    pub const fn example(name: &'static str) -> Self {
        Self::built(Target::Example, name)
    }

    /// The binary `name`, built in the test package's folder until [`in_dir`](Self::in_dir) names another.
    pub const fn bin(name: &'static str) -> Self {
        Self::built(Target::Bin, name)
    }

    /// The ELF at `path`, which the test or its project built.
    pub const fn elf(path: &'static str) -> Self {
        Self {
            source: Source::Elf(path),
            dir: ".",
            features: &[],
        }
    }

    const fn built(target: Target, name: &'static str) -> Self {
        Self {
            source: Source::Built(target, name),
            dir: ".",
            features: &[],
        }
    }

    /// The firmware built in `dir`, the folder that holds its `Cargo.toml` and `.cargo/config.toml`. A relative
    /// `dir` is relative to the test package's folder, where cargo runs its tests.
    pub const fn in_dir(self, dir: &'static str) -> Self {
        Self { dir, ..self }
    }

    /// The firmware built with `features` on.
    pub const fn features(self, features: &'static [&'static str]) -> Self {
        Self { features, ..self }
    }

    /// The firmware's ELF. Builds it the first time in this process, and finds it again after that.
    ///
    /// Fails with [`Error::Build`] when cargo fails, and with [`Error::NoArtifact`] when cargo builds no target
    /// of that name.
    pub fn elf_path(&self) -> Result<PathBuf, Error> {
        let (target, name) = match self.source {
            Source::Elf(path) => return Ok(PathBuf::from(path)),
            Source::Built(target, name) => (target, name),
        };
        let mut built = BUILT.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, elf)) = built.iter().find(|(firmware, _)| firmware == self) {
            return Ok(elf.clone());
        }
        let elf = build(Path::new(self.dir), target, name, self.features)?;
        built.push((*self, elf.clone()));
        Ok(elf)
    }

    /// A console on the firmware, just after it starts, with its banner read. It sends `reset` to the board's
    /// console first. When the next banner names the firmware's image ID, the board already runs it, and nothing
    /// is flashed. Otherwise it flashes the firmware. A reset is quicker than a flash, and keeps the USB link. The
    /// console holds the board until it is dropped.
    pub fn console(&self) -> Result<Console, Error> {
        let held = hold_board();
        let runner = Runner::new(Image::from_elf(self.elf_path()?)?)?;
        let client = runner.console(&board(&runner)?)?;
        Ok(Console::new(client, held))
    }

    /// A console on the firmware, just after it is flashed and started, even when the board already runs it. The
    /// first boot after a flash follows a probe's reset.
    pub fn fresh_console(&self) -> Result<Console, Error> {
        let held = hold_board();
        let runner = Runner::new(Image::from_elf(self.elf_path()?)?)?;
        let client = runner.fresh_console(&board(&runner)?)?;
        Ok(Console::new(client, held))
    }
}

/// The value of the environment variable `name`, or `None` when it is unset or empty. `None` also writes why to
/// the process's stderr, which shows without `--nocapture`. The test that returns early still reports `ok`,
/// because libtest cannot mark a test as skipped while it runs.
pub fn env_or_skip(name: &str) -> Option<String> {
    let value = set(std::env::var(name).ok());
    if value.is_none() {
        // libtest captures `eprintln!` and shows it only for a failed test. A write to the stderr handle goes past
        // that capture, so the skip shows for a test that passes.
        skipped(name, std::io::stderr());
    }
    value
}

/// Writes that a test is skipped because `name` is not set. A failed write is ignored, as `eprintln!` would not
/// be.
fn skipped(name: &str, mut to: impl std::io::Write) {
    let _ = writeln!(to, "skipped: {name} is not set");
}

/// `value`, when it is there and not empty.
fn set(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

/// The ELF of each firmware built in this process.
static BUILT: Mutex<Vec<(Firmware, PathBuf)>> = Mutex::new(Vec::new());

/// Builds `target` `name` in `dir` with `features`, and returns its ELF.
fn build(dir: &Path, target: Target, name: &str, features: &[&str]) -> Result<PathBuf, Error> {
    use std::io::Read;
    use std::process::Stdio;
    let failed = |message: String| Error::Build {
        dir: dir.to_owned(),
        message,
    };
    let mut cargo = cargo::command(dir, target, name, features)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| failed(format!("cargo did not start: {error}")))?;
    // cargo's progress shows on the terminal while it builds, and its errors stay for `Error::Build`. A second
    // thread drains stderr, so a full pipe cannot stop cargo while stdout is read here.
    let stderr = cargo.stderr.take();
    let errors = std::thread::spawn(move || {
        stderr
            .map(|stderr| cargo::tee(stderr, std::io::stderr()))
            .unwrap_or_default()
    });
    let mut stdout = String::new();
    if let Some(mut out) = cargo.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let status = cargo
        .wait()
        .map_err(|error| failed(format!("cargo did not finish: {error}")))?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        return Err(failed(errors));
    }
    cargo::artifact(&stdout, target, name).ok_or_else(|| Error::NoArtifact {
        name: name.to_owned(),
        dir: dir.to_owned(),
    })
}

/// One test at a time talks to the board. A test that panicked while it held the board leaves the board to the
/// next test.
static BOARD: Mutex<()> = Mutex::new(());

fn hold_board() -> MutexGuard<'static, ()> {
    BOARD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The board that `MCU_DEVICE` names, or else the one board that `runner`'s image can run on.
fn board(runner: &Runner) -> Result<Board, Error> {
    let device = set(std::env::var("MCU_DEVICE").ok());
    Ok(runner.board(device.as_deref())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_firmware_is_built_from_the_test_s_folder_until_another_is_named() {
        let console = Firmware::example("console");
        assert_eq!(console.source, Source::Built(Target::Example, "console"));
        assert_eq!(console.dir, ".");
        assert!(console.features.is_empty());
        let app = Firmware::bin("app").in_dir("../fw").features(&["wifi"]);
        assert_eq!(app.source, Source::Built(Target::Bin, "app"));
        assert_eq!(app.dir, "../fw");
        assert_eq!(app.features, ["wifi"]);
    }

    #[test]
    fn an_elf_built_elsewhere_is_used_as_it_is() {
        assert_eq!(
            Firmware::elf("/t/app.elf").elf_path(),
            Ok(PathBuf::from("/t/app.elf"))
        );
    }

    #[test]
    fn a_firmware_built_before_in_the_process_is_not_built_again() {
        // The folder does not exist, so a build would fail.
        let cached = Firmware::example("cached").in_dir("/no/such/folder");
        let other = Firmware::example("cached").in_dir("/no/such/other");
        BUILT
            .lock()
            .unwrap()
            .push((cached, PathBuf::from("/t/cached")));
        assert_eq!(cached.elf_path(), Ok(PathBuf::from("/t/cached")));
        assert!(matches!(other.elf_path(), Err(Error::Build { .. })));
    }

    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(set(Some("home".to_owned())), Some("home".to_owned()));
        assert_eq!(set(Some(String::new())), None);
        assert_eq!(set(None), None);
    }

    #[test]
    fn a_skip_names_the_variable_on_a_line_of_its_own() {
        let mut out = Vec::new();
        skipped("MCU_WIFI_SSID", &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "skipped: MCU_WIFI_SSID is not set\n"
        );
    }

    #[test]
    fn an_unset_variable_skips_and_a_set_one_gives_its_value() {
        assert_eq!(
            env_or_skip("MCU_SCENARIO_NEVER_SET_IN_ANY_ENVIRONMENT"),
            None
        );
        assert_eq!(
            env_or_skip("PATH"),
            std::env::var("PATH").ok().filter(|path| !path.is_empty())
        );
    }
}
