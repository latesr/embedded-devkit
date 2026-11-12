# ADR-0003: `mcu-run` over the probe-rs library

**Status: accepted**

Covers: `mcu-run`, and the `mcu-run.x` that each board crate's `build.rs` writes.

## Context

A host test needs a running board: flashed with the firmware under test, reset, and with its log readable while the test sends commands to the console. A developer needs the same steps from `cargo run` and `cargo test`. An embedded-test binary needs a runner that resets the chip before each test and reads its result. The host tests of every repo compile against the crate that gives these steps, so its API must stay stable across probe-rs releases. The probe-rs binary does each step, but a host test cannot call a binary as a function.

## Decision

- `mcu-run` uses the probe-rs library, not the probe-rs binary.
- The library's API holds no probe-rs type, because probe-rs changes its API at each 0.x release.
- `mcu-run` implements its own embedded-test runner over the semihosting commands of the library, because probe-rs keeps its runner in the crate of its binary.
- `mcu-run` holds no knowledge of any one board. The firmware names its chip, and any preferred flash algorithm, in `(INFO)` sections of its ELF, which the board crate's `mcu-run.x` writes. The probe-rs binary reads the same chip section.
- To pick one board from several, `mcu run` reads only each probe and the IDCODE of each TAP, so it stops no chip. It matches each IDCODE against the chips that probe-rs identifies by IDCODE alone.
- When a probe-rs release describes a chip wrongly, the board crate supplies a chip description, so probe-rs does not need a patch.
- `mcu-run` finds another process on the board's console port from the host's list of open files, not from an open of the port.

## Rejected

- **The probe-rs binary in a separate process.** A host test would read the binary's output to learn the board's state.
- **The chip in the runner line of `.cargo/config.toml`.** Each application would repeat the board's chip and flash algorithm, and a host test that flashes through the library would repeat them again.
- **A Rust static in a named section, as the `probe-rs-meta` crate writes it.** The linker puts the section in a loaded segment, so on the NUCLEO-H7S3L8 each flash writes another page of the internal flash.
- **The auto-detection of probe-rs.** It attaches to the chip, halts an Espressif core that it cannot identify by IDCODE, and fails on the STM32H7S3.
- **The error from opening the port.** A run and an on-target test do not open the port, and a terminal program that does not lock the port lets the open succeed.

## Consequences

- A move to a new probe-rs version changes `mcu-run` only.
- A new embedded-test format needs a change in `mcu-run`.
- An application on a board crate adds `-Tmcu-run.x` to its link arguments, or passes `--chip`.
- Two ST-LINKs need a device value, because probe-rs cannot identify an STM32H7RS from its probe. Two attached boards from the ESP32, the ESP32-S2, and the ESP32-S3 also need one, because the three have one IDCODE.
- A test fails before the flash while a terminal has the board's console open.
- The on-target tests run the installed `mcu` binary, so after a change to `mcu-run` the binary is installed again before the hardware tests.
