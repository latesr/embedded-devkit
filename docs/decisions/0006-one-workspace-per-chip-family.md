# ADR-0006: One workspace for each chip family

**Status: accepted**

Covers: the `esp/` and `stm32/` workspaces, the family crate `embedded-devkit-esp`, and how `cargo xtask check` reaches them.

## Context

The root workspace holds the host crates. A board crate builds for its microcontroller, and the ESP32 board crates need a newer toolchain than the host crates. esp-hal does not build for the host, so a workspace that held both could not run `cargo test` or `cargo clippy` over its members. A workspace has one lock file and one table of shared dependencies. Every crate from the esp-hal repository must come from one revision, because two copies of esp-rom-sys cannot link. Most of the code for the ESP32-C5 is also the code for the ESP32-S31.

## Decision

- Each chip family has one workspace outside the root, `esp/` and `stm32/`, which the root `Cargo.toml` excludes. The family's manifest names the HAL revision once, and the workspace holds the one lock file, so the boards of a family build against one HAL.
- Each family workspace copies the root's lint table, because a crate outside the root workspace cannot inherit it.
- A family crate holds what the board crates of one chip family share, behind one feature for each chip. A board crate is the family crate for its chip, plus what its board adds.
- A family crate exists once a family has two boards. Until then, the one board crate holds its own code, as the NUCLEO-H7S3L8 crate does.
- The ESP32 examples are written once, in `esp/examples/`, because the copy for each board differed only in two names. An example cannot be in the family crate, because it runs on one board, with that board crate's `.cargo/config.toml`.
- The root `Cargo.toml` lists the board and family folders, and `cargo xtask check` runs the checks of each folder in that folder, where its own toolchain and target apply. `AGENTS.md` names the lists and the checks.

## Rejected

- **Every crate in the root workspace.** esp-hal does not build for the host, so the test and lint commands of the workspace would fail, and the host crates would take the toolchain and the HAL pin of the board crates.
- **One workspace for each board crate.** Two ESP32 workspaces would hold two HAL pins and two lock files, which would move apart.
- **A copy of the common code in each ESP32 board crate.** A change would be made in two places, and the two copies would drift.

## Consequences

- A new ESP32 board is a chip feature in `embedded-devkit-esp`, a thin board crate, and a row in each of the two lists.
- cargo-mutants does not reach the board crates or the family crate, because their tests run on the board. Their on-target tests stand in for it.
