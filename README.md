# embedded-devkit

Tools to test firmware on a microcontroller from the host, and board crates to run them on. A host test flashes the firmware, then drives it through a text console and checks the replies.

## Crates

| Crate | Runs on | Contents |
|---|---|---|
| [`mcu-console`](mcu-console/README.md) | The device | A line console: the application supplies its commands, and a person or a host test sends lines |
| [`mcu-console-net`](mcu-console-net/README.md) | The device | Network commands for the console, over embassy-net |
| [`mcu-console-perf`](mcu-console-perf/README.md) | The device | `perf run`, which measures one of the application's commands |
| [`mcu-run`](mcu-run/README.md) | The host | The `mcu` command and its library: flash, run, read defmt, and open the console |
| [`mcu-scenario`](mcu-scenario/README.md) | The host | The harness for scenario tests, which drive running firmware through its console |

The board crates bring up a reference board for these tools:

- ESP32-C5-DevKitC-1 v1.2, in `esp/embedded-devkit-esp32-c5-devkitc-1`
- [ESP32-S31-Function-CoreBoard-1 v1.0](esp/embedded-devkit-esp32-s31-function-coreboard-1/README.md)
- [NUCLEO-H7S3L8](stm32/embedded-devkit-nucleo-h7s3l8/README.md)

[`REQUIREMENTS.md`](REQUIREMENTS.md) states what each crate must do. [`docs/`](docs/README.md) holds the Architectural Decision Records (ADRs).

## Start

1. Install the host tool: `cargo install --path mcu-run --locked`.
2. Attach a reference board by its debug USB port.
3. In the board crate's folder, run `cargo run --release --example console -- --console`.

The board crate's `.cargo/config.toml` names `mcu run` as the runner. `mcu` finds the board from the chip in the ELF. With two boards of the same chip attached, `-d probe://<part of the probe selector>` names one, and `mcu list` shows the selectors.

## Roadmap

The tools work on a board that a debug probe reaches. The aim is a product device that the same tests reach in the field, with the debug port off. Each heading gives the state of its item.

### A device: partly built

A device is the unit that a test runs on. It has four parts:

- an optional probe
- a console link: USB serial, TCP, or NATS
- a deployer, which puts a new image on the device
- a log source, which sends the defmt log to the host

| Part | Built | Planned |
|---|---|---|
| Probe | A probe-rs probe | Nothing more |
| Console link | USB serial | Plain TCP, and NATS over mutual TLS |
| Deployer | A flash through the probe | An over-the-air update, then a reboot |
| Log source | RTT through the probe | A UART through a USB-UART bridge, then the network |

`-d` and `MCU_DEVICE` name the device. They take one form, `probe://<text>`. A bare word is reserved for an entry in a devices file. The planned file is in the user's own configuration, never in a repo, and gives each device's four parts.

### The image in the banner: built

The console banner names the running image, so a host knows which build answers on any link. `mcu-run` reads the same ID from the ELF. An ESP32 image's ID is the SHA-256 of its ELF file, which espflash writes into the app descriptor. The NUCLEO-H7S3L8's ID is the GNU build ID.

An ESP32 board crate reads the ID through a symbol in its linker script. When esp-hal's `EspAppDesc::app_elf_sha256()` reads the field correctly, the board crate uses that accessor instead.

### A device without a probe: planned

A provisioned device has its debug port off. A new image then reaches the device by an over-the-air update and a reboot, and the banner tells the host that the new image runs. The scenario tests do not assume a probe, so they run on such a device unchanged. The on-target tests use semihosting, so they need a probe. The update mechanism is a later requirement.

### The console over the network: planned

The console runs over plain TCP and over NATS. Over NATS, a device opens a mutual-TLS connection to a server. An operator then reaches its console without an open port on the device.

### The log: partly built

A build has one global defmt logger. The built logger writes to RTT.

The next logger writes to a UART in espflash's frame format, and `mcu-run` reads a UART log that mixes text and frames. Later, a ring buffer holds the log, and a sender drains it to any `embedded_io::Write`: a UART, a TCP socket, or NATS. The frames stay in rzcobs encoding, so a reader can find the next frame after lost bytes.

## Licence

MIT or Apache-2.0, at your option.
