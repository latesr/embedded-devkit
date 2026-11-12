# Docs

Covers: the index of the ADRs.

| ADR | Summary |
|---|---|
| [ADR-0001: The synced copies of the shared rules and checks](decisions/0001-synced-shared-rules-and-checks.md) | Each repo holds a copy of the rule files and `xtask/src/shared/`, and the devkit's sync keeps it current |
| [ADR-0002: The line console protocol](decisions/0002-line-console-protocol.md) | One text protocol, the version and the image ID in the banner, one command trait, writes that do not wait, and a cancel signal |
| [ADR-0003: `mcu-run` over the probe-rs library](decisions/0003-mcu-run-over-probe-rs.md) | A library and a thin binary, its own embedded-test runner, the same steps on every board, and a run that follows the board across a reset |
| [ADR-0004: Scenario tests through the console](decisions/0004-scenario-tests-through-the-console.md) | Scenario tests on the host through the console, on-target tests under embedded-test, one selector for the probe and the port, and `cargo xtask hil` on one board crate |
| [ADR-0005: The minimal board crate](decisions/0005-minimal-board-crate.md) | The HAL's peripherals handed to the application, what the crate adds beside them, capabilities as functions, and no traits from the repos it serves |
| [ADR-0006: One workspace for each chip family](decisions/0006-one-workspace-per-chip-family.md) | One HAL pin and one lock file per family, the family crate behind a chip feature, and how the check reaches each folder |
| [ADR-0007: `unsafe` in the board crates](decisions/0007-unsafe-in-board-crates.md) | The three sites and their missing HAL APIs, a HAL patch over a register read, and review of what a macro expands to |
| [ADR-0008: The boot count](decisions/0008-boot-count.md) | A count that restarts after any other reset, a marker where the reset reason cannot tell a probe's reset apart, and the backup register that was rejected |
| [ADR-0009: The network commands over embassy-net](decisions/0009-network-commands-over-embassy-net.md) | One crate for the network commands, the stack calls in one module, joining as a console command, and the `Wifi` trait |
