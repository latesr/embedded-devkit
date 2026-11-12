# mcu-scenario

Scenario tests for firmware that runs an [mcu-console](../mcu-console/README.md) console. A scenario test is a host Rust test that drives the running firmware through its console, as a person at a terminal would. It runs on [mcu-run](../mcu-run/README.md).

- It builds the firmware's example or binary with cargo, in the firmware's own folder and with its own toolchain, once per test process. The build takes the test's environment, so a variable that the firmware reads with `env!` comes from the shell that runs `cargo test`. cargo's progress shows while it builds.
- It takes the board from `MCU_DEVICE`, such as `probe://0483:3754`, or else picks the attached board that the firmware's chip runs on.
- It first sends `reset` to the board's console. When the board already runs the firmware, which its banner's image ID shows, the reset restarts it and nothing is flashed. Otherwise it flashes the firmware. A reset is quicker than a flash, and keeps the USB link. So the first test of a process sees `reset: software` in `info` when the board already ran the firmware, and `reset: debugger` after a flash.
- One test at a time holds the board, so a test file does not need `--test-threads=1`.
- It refuses firmware with another console protocol version, so a firmware and a host from different releases fail at once.

```rust
use mcu_scenario::{CommandResult, Firmware};

static CONSOLE: Firmware = Firmware::example("console").in_dir("../firmware");

#[test]
#[ignore = "needs a board"]
fn info_names_the_board() {
    let mut console = CONSOLE.console().unwrap();
    let reply = console.send("info").unwrap();
    assert_eq!(reply.result, CommandResult::Ok);
    assert_eq!(reply.field("board"), Some("esp32-s31-function-coreboard-1"));
}
```

`cargo test -- --ignored` runs the tests with a board attached. The crate needs Rust 1.95, so the package that holds the scenario tests states `rust-version = "1.95"` or later, apart from the firmware's own MSRV. `Firmware::fresh_console` flashes even when the board already runs the firmware. `env_or_skip` returns early from a test that needs a variable that is not set, such as a Wi-Fi password. It writes the skip to stderr, where it shows without `--nocapture`, and the test reports `ok`.

`Console::send_until` sends a line until its reply shows a state, such as a lease in `net config`, and returns what the test takes from that reply. A helper takes the console as `&mut Console`. `host_address` gives the address of this host on the board's subnet, for a test where the board connects back to the host.

The requirements it meets are H-7 to H-13 in the [embedded-devkit requirements](../REQUIREMENTS.md).

## Licence

MIT or Apache-2.0, at your option.
