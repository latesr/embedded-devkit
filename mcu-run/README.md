# mcu-run

Flashes a board, runs the firmware, and drives its console from the host. It is a library for host tests and a binary, `mcu`, for the command line and for `cargo run`.

- It uses the probe-rs library. It finds a board by part of its probe-rs selector, `VID:PID:serial`, given as `probe://<text>`, or else by the ELF's chip. It finds the board's serial port from the same serial number.
- It flashes, resets, blanks the RTT control block, and runs, the same on every board. It reads RTT and decodes defmt without another tool.
- It stops at a line that contains a given string, at a deadline, or when the firmware exits through semihosting, and exits with a code that says which.
- It follows the board across a software reset or the reset button, writes `(the board reset)`, and continues with the next boot's output.
- It sends console lines and reads the body and the result line, for a host test or a person.
- It gives a host test a console on freshly started firmware. It restarts the firmware through the console when the console's banner names the ELF's image ID, and otherwise flashes it.

The requirements it meets are M-1 to M-26 in the [embedded-devkit requirements](../REQUIREMENTS.md).

## The command

`cargo install --path mcu-run` puts `mcu` on the path. An application's `.cargo/config.toml` names it as the runner:

```toml
runner = "mcu run"
rustflags = ["-C", "link-arg=-Tmcu-run.x"]
```

`mcu run` takes the chip from the ELF's `.probe-rs.chip` section. A devkit board crate's `build.rs` writes `mcu-run.x`, the linker script that puts the section there, and the application links it with `-Tmcu-run.x`. On a board where probe-rs would choose the wrong flash algorithm, the script names the right one in a `.mcu-run.flash-algorithm` section. `--chip` and `--prefer-flash-algorithm` name them for an ELF that does not hold them.

The script also names the image's ID in flash, where the firmware reads it for the console's banner. `Image::id` reads the same ID from the ELF. An ESP32 image's ID is the SHA-256 of its ELF file, which espflash writes into the app descriptor. The NUCLEO-H7S3L8's is the GNU build ID, so an application on it also links with `--build-id=sha1`, and the link fails without it.

An ESP32 boots in QIO, the fastest flash mode its bootloader sets. A board that cannot run its flash in quad mode does not boot in QIO. Flash it once with `--flash-mode dio`, and name `dio` in a `.mcu-run.flash-mode` section of its `mcu-run.x`, so every later run takes it.

`cargo run` then flashes and runs the binary, and `cargo test` runs an embedded-test binary test by test. `mcu run` takes `--until <text>`, `--timeout <seconds>`, and `--log <file>` after the ELF, where `cargo run -- <options>` puts them. The other commands are:

- `mcu console` opens the board's console in the terminal, asks it for a fresh prompt, and connects again after a reset that takes the port away. Ctrl-] or Ctrl-D exits it.
- `mcu run --console`, or `cargo run -- --console`, flashes the firmware and opens its console in place of its log.
- `mcu console --send <line>` sends one line and prints the reply.
- `mcu list` shows the boards, with any process that holds a board's port. `mcu list -c` also opens each probe and shows the chip at it, where the chip's JTAG IDCODE names it.

Before a flash or a reset, `mcu` fails when another process holds the board's console port, and names that process. A test then does not share the board with an open terminal. The check uses `lsof`, and a host without `lsof` skips the check.

With several boards and no device, `mcu run` keeps the boards with a probe that can debug the ELF's chip. It reads the IDCODEs when more than one board is left. When that is not enough, such as with two boards of the same chip, `-d probe://<text>` names one board by a substring of its selector. `--device` is the long form of `-d`. `run` and `console` take the value from the `MCU_DEVICE` environment variable when the option is not given, so `MCU_DEVICE=probe://<part of the selector> cargo run` reaches the runner without `-- -d`.

Other forms of the device value are reserved. A `<scheme>://` other than `probe://` names a network console, and a bare name names an entry in a devices file. `mcu` does not support either form yet.

## The library

A host test starts the firmware and talks to its console:

```rust
use mcu_run::{CommandResult, Image, Runner};
use std::time::Duration;

let runner = Runner::new(Image::from_elf("target/riscv32imafc-unknown-none-elf/release/examples/console".into())?)?;
let board = runner.board(std::env::var("MCU_DEVICE").ok().as_deref())?;
let mut console = runner.console(&board)?;
let reply = console.send("info", Duration::from_secs(5))?;
assert_eq!(reply.result, CommandResult::Ok);
```

## Licence

MIT or Apache-2.0, at your option.
