@CONVENTIONS.md
@CONTRIBUTING.md
@CODE.md
@TESTING.md

# embedded-devkit

embedded-devkit holds the tooling and board platforms that related repos share and build on. A derived repo takes copies of the shared files through `cargo xtask sync`.

## Shared files

`CONVENTIONS.md`, `CONTRIBUTING.md`, the rule files that `CONTRIBUTING.md` indexes, `xtask/src/shared/`, and the git hooks are the originals. Each derived repo holds a copy.

1. Edit the originals here, and commit.
2. Run `cargo xtask sync ../<derived-repo>`. Sync copies from the devkit's `HEAD`, refuses while the shared files have changes that are not committed, and installs the hooks from the shared `hooks` module.
3. Commit the copy in each repo. The subject names the devkit commit, such as "Sync the rule files and xtask/src/shared from embedded-devkit dd579ec".

## Published crates

`mcu-console`, `mcu-console-net`, `mcu-console-perf`, `mcu-run`, and `mcu-scenario` are intended to be published on crates.io.

- `REQUIREMENTS.md` states what keeps each able to move to its own repo. One moves out when another repo wants it at a tag without the board crates, or when a hardware CI failure blocks a console change.
- Each holds a copy of `LICENSE-MIT` and `LICENSE-APACHE` from the repo root, because `cargo package` takes files from the crate's own folder only.
- `mcu-run` turns off `serialport`'s `libudev` feature, so a Linux host does not need a system package to build it.
- `mcu-console-net` and `xtask` take embedded-iperf2 from latesr/embedded-iperf2 on GitHub, at one commit that both name.

## The board crates

Each board crate brings up one reference board. Run its commands in its folder, where its `rust-toolchain.toml` and `.cargo/config.toml` choose the target:

- `cargo test --release --test smoke` runs the on-target tests through `mcu-run`.
- `cargo run --release --example <name>` runs an example. `-- --console` after a console example opens its console.
- An example or a test behind a feature needs that feature, as its `required-features` in the crate's `Cargo.toml` states.

The boards:

- **ESP32-C5-DevKitC-1**, in `esp/embedded-devkit-esp32-c5-devkitc-1/`. It is the ESP32 family crate with the `esp32c5` feature, plus the board's PSRAM settings and its name in `info`.
- **ESP32-S31-Function-CoreBoard-1**, in `esp/embedded-devkit-esp32-s31-function-coreboard-1/`. It is the family crate with the `esp32s31` feature, plus the board's PSRAM settings and its Ethernet pins. The Ethernet scenario tests need its Ethernet port cabled to a network with DHCP that this host reaches.
- **NUCLEO-H7S3L8**, in `stm32/embedded-devkit-nucleo-h7s3l8/`. The program lives in external flash, and the crate's `build.rs` writes the `memory.x` that puts it there. The same `build.rs` names `mx25uw25645g_stm32h7s38-nucleo` as the flash algorithm in `mcu-run.x`, because probe-rs also offers the STM32H7S78-DK's algorithm for the same flash.

A family crate holds what the board crates of one chip family share. `esp/embedded-devkit-esp/` is the ESP32 one, and builds only with one chip feature on. The ESP32 board crates share their examples, in `esp/examples/`. Each crate lists the examples it builds in its `Cargo.toml`, and its `examples/board.rs` names the board for them.

The ESP32 crates build against esp-hal's `integration` branch on latesr/esp-hal, which holds the changes that the devkit needs before esp-hal releases them. `esp/Cargo.toml` names the commit of one tag on it, once, for both chips.

## The hardware tests

`cargo xtask hil --board <folder>` runs the hardware tests on one board crate's board:

1. It builds the crate's examples, with every feature.
2. It runs the crate's on-target tests, with every feature, so a test behind a feature runs.
3. It runs `scenario-tests`, which drive the examples through `mcu-scenario` and the console. It passes the folder to them in `MCU_BOARD`.

- The scenario tests' table of board crates lists each crate's features. The list must match every feature the crate has, or the tests build the examples again with other features.
- `--board` is needed when more than one board crate is listed.
- The scenario tests are `#[ignore]`, so `cargo xtask check` compiles them and does not run them.
- `-d probe://<part of the selector>` names the board when two boards have the same chip, and `hil` passes it to `mcu-run` and the scenario tests in `MCU_DEVICE`. Both ESP32 boards are `303a:1001`, so a selector for one of them includes part of its MAC.
- The Wi-Fi scenario tests join the network that `MCU_WIFI_SSID` and `MCU_WIFI_PASSWORD` name. With either unset, or on a board without Wi-Fi, they return without testing. The password is never written to a file or a log.
- The iperf scenario tests run embedded-iperf2's server as the host end, through `cargo xtask iperf-server` from `target/debug/xtask`. The Mac's application firewall asks once to allow that path.
- `cargo xtask hil` reports unset variables.

A new board behaviour starts with a failing scenario test or on-target test, run through `cargo xtask hil`.

The on-target tests and `cargo run` use the installed `mcu` binary from `mcu-run`, which each board crate's `.cargo/config.toml` names as its runner. The scenario tests use the `mcu-scenario` and `mcu-run` libraries, which the build compiles from the tree. After a change to `mcu-run` or its dependencies, run `cargo install --path mcu-run --locked` before the hardware tests, or the on-target tests run the old binary.

## Checks this repo adds

- **MSRV.** The check runs each crate at its own `rust-version`, one `cargo +<version> check` per version, because the crates in the Platforms table of `REQUIREMENTS.md` have different ones. A crate without its own `rust-version` is checked at the workspace's. The ESP32 board crates are checked at 1.95 and the NUCLEO-H7S3L8 board crate at 1.91, for their own targets. Install the toolchains with `rustup toolchain install 1.91 1.95 --profile minimal`, then `rustup target add riscv32imac-unknown-none-elf riscv32imafc-unknown-none-elf --toolchain 1.95` and `rustup target add thumbv7em-none-eabihf --toolchain 1.91`.
- **Cross targets.** A `no_std` crate sets `[package.metadata.devkit] cross = true` in its `Cargo.toml`, and the check builds it for every target in `rust-toolchain.toml`. A host crate such as `mcu-run` does not set it, so it is never built for a microcontroller.
- **Board workspaces.** A board crate is outside the root workspace, because it builds for a microcontroller and esp-hal does not build for the host. The ESP32 board crates share the workspace in `esp/`, so that both chips build against one esp-hal and one lock file. The STM32 board crates share the workspace in `stm32/`. The root `Cargo.toml` excludes both, and lists each board crate's folder in `[workspace.metadata.devkit] boards`. The check runs fmt, clippy, a build of every target, the docs, and a check at the crate's own `rust-version` in each listed folder.
- **Family crates.** The root `Cargo.toml` lists each family crate's folder in `[workspace.metadata.devkit] families`, so `cargo xtask hil` never takes it for a board. Its `[package.metadata.devkit] chips` maps each chip feature to its target. The check runs clippy, the docs, and the MSRV check on its library once for each chip.
- **Target folders.** Each listed folder builds in its own `target` folder, because each crate's `.cargo/config.toml` sets esp-hal's configuration, and a shared folder rebuilds esp-hal for each crate.

The tests of a board or family crate run on the board, so the check does not run them, and cargo-mutants does not reach them.

## The template

`template/` is a cargo-generate template for a new repo. It is not built as part of this workspace. To start a repo beside the others, in the folder that holds the devkit:

```
cargo generate --path embedded-devkit/template --name <repo> --define "description=<one sentence>"
git -C <repo> branch -m main
cd embedded-devkit
cargo xtask sync ../<repo>
```

Sync supplies the rule files, `xtask/src/shared/`, and the git hooks, so the template does not hold them. cargo-generate runs `git init` in the new repo, so the hooks have a folder to go in. It names the branch `master`, and the CI workflow builds `main`. After generation the new repo owns every other file.

Before committing a change to the template, generate a repo from it in a scratch folder, sync it, and run `cargo xtask check` there.
