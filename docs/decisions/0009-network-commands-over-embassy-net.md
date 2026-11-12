# ADR-0009: The network commands over embassy-net

**Status: accepted**

Covers: `mcu-console-net`, and the `Wifi` implementations in the board crates.

## Context

A library's hardware test needs the board on a network, and checks the network from the console: the address, a name lookup, a ping, the time of a TCP connect, and the throughput. The commands must be the same on every board, on Wi-Fi or on Ethernet. On Wi-Fi, the board must join a network first. The network's name and password belong to the person who runs the test, not to the repo. Each board's network is an embassy-net `Stack`, which does not report the medium or the MTU, and does not join a network.

## Decision

- The `net` commands are in a crate of their own, `mcu-console-net`, over `mcu-console` and embassy-net. An application without a network then does not build a network stack.
- The commands call embassy-net's `Stack` directly, because embassy-net is the one stack that the boards use. Every call is in the `probe` module, so a later trait over IP stacks changes one file.
- The `net wifi` commands run over a `Wifi` trait, which each board crate with a radio implements. `()` implements it for a board without a radio, so an application builds `NetCommands` in the same way on either.
- After the link drops, the task that owns the radio joins the same network again. `net wifi drop` asks that task to drop the link, so a test checks the rejoin through the path of a real drop.
- The host tests wire two embassy-net stacks together in one process, with a fake radio that implements `Wifi`, so cargo-mutants covers the crate without a board.

## Rejected

- **The `net` commands in `mcu-console`.** Every application would build a network stack.
- **The `net` commands in each board crate.** The same commands would be written once for each board.

## Consequences

- `mcu-console-net` sets smoltcp's DNS result count and server count. Every crate in the same application gets those values, because smoltcp's build script refuses two different values.
- `mcu-console-net` cannot be published before embedded-iperf2 is.
- A new radio needs a `Wifi` implementation in its board crate.
- The Wi-Fi scenario tests test nothing until `MCU_WIFI_SSID` and `MCU_WIFI_PASSWORD` are set, and `cargo xtask hil` says so.
