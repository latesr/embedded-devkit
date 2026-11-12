# ADR-0004: Scenario tests through the console

**Status: accepted**

Covers: `mcu-scenario`, `scenario-tests`, `cargo xtask hil`, and `mcu-run`'s `Client`, `Runner::console`, and `Runner::fresh_console`.

## Context

A library such as a vault or a network client needs hardware tests that talk to running firmware, across resets and over time. A vault test writes, resets the chip, and reads back. In embedded-test, a test ends at the next reset, and the host learns only whether each test exited cleanly.

## Decision

- A test that checks one thing on the chip, between one reset and its exit, is an on-target test under embedded-test. A test across a reset or over time is a scenario test, through the console that a person uses ([ADR-0002](0002-line-console-protocol.md)).
- Every project's scenario tests share one harness, `mcu-scenario`.
- `mcu-scenario` takes the ELF's path from cargo's messages, because the path depends on the workspace, the target, and the profile.
- The banner is the record of what the board runs. `Runner::console` compares its image ID with the ELF's, so a second test process does not need state in `mcu-run`.
- On each reference board, the probe and the console port are one USB device with one serial number, so one device value, `probe://<part of its selector>`, names both.
- `cargo xtask hil` runs one board crate's on-target tests before its scenario tests, because the on-target tests prove that the board works. It passes the board crate's folder in `MCU_BOARD`, so one suite runs on every board crate.
- A scenario test that needs a peer on the host runs the peer from the repo of the crate under test, through an `xtask` command, such as `cargo xtask iperf-server`. That repo tests the peer's interop with other implementations, so the devkit does not repeat those tests. `mcu-run` then does not depend on the crate under test.

## Rejected

- **Every hardware test under embedded-test.** Such a test cannot check what happens after a reset.
- **Scenario tests in each board crate.** A board crate's workspace builds for the chip, and a host test does not build there.
- **A harness in each repo.** The devkit and a library repo each wrote one, and each found ELF paths, picked the board, and parsed replies in its own way.
- **The harness as a module of `mcu-run`.** `mcu-run` is the transport, and the harness is test policy: the environment, the build, and the lock. A crate of its own keeps cargo out of `mcu-run`.
- **A fixed ELF path under `target/`.** Each project lays out `target/` in its own way, and the test fails when the firmware is not built yet.
- **A value that each test keeps, to tell `Runner::console` what the board runs.** Each test binary would keep it in a static, and every user of `mcu-run` would write the same bookkeeping.
- **iperf 2.2.2 as the host end.** Each host would need it installed, the tests would parse its CSV output, and the tests would repeat embedded-iperf2's own interop tests.
- **The peer in a thread of the test process.** A test binary takes a new path at each build, and the Mac's application firewall holds back the first datagrams of each new flow into such a binary.
- **`mcu-run iperf -s` as the peer.** `mcu-run` could then be published only after embedded-iperf2.
- **One `hil` run over every board crate.** A `probe://` value cannot name two boards with the same USB ID, and both ESP32 boards are `303a:1001`.

## Consequences

- A scenario test needs an attached board, so it is `#[ignore]`. `cargo xtask hil` runs it, and `cargo xtask check` only compiles it.
- A scenario test builds its firmware with the features that `cargo xtask hil` builds with, or cargo builds the firmware again.
- A new board crate adds its row to the table of the scenario tests.
- A device value names a board by its probe. A board without a probe, or with its console port on another USB device, cannot be named yet. The README's roadmap states the plan.
- The peer runs from `target/debug/xtask`, a path that the Mac's application firewall asks about once.
