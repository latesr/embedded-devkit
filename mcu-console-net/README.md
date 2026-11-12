# mcu-console-net

Network commands for [mcu-console](../mcu-console/README.md): the device's address, DNS lookups, ping, the round trip of a TCP connect, an iperf 2 throughput test, and joining a Wi-Fi network. The iperf test is a bring-up and test command, so only the `iperf` feature adds it. The feature is off by default.

- The crate is `no_std` and does not allocate.
- The commands run over an embassy-net `Stack`, so they are the same on every board.
- A board crate implements the `Wifi` trait over its radio for the `net wifi` commands.
- The crate sets smoltcp's `dns-max-result-count-4` and `dns-max-server-count-3` for every build it is in. smoltcp's build script fails with "multiple values set for feature" when another crate sets a different count, so an application's other crates must set the same values or none.

The crate documentation lists the commands and their output. The requirements it meets are N-1 to N-17 in the [embedded-devkit requirements](../REQUIREMENTS.md).

## Licence

MIT or Apache-2.0, at your option.
