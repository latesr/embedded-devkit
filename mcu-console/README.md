# mcu-console

A line console for a microcontroller. The application supplies its commands, and a person at a terminal or a test runner on the host sends lines and reads the replies.

- The core is `no_std`. It does not allocate or do I/O.
- A blocking driver runs it over `embedded-io`. An async driver runs it over `embedded-io-async`.
- The transport is whatever the driver's `embedded-io` implementation reaches: a UART, USB Serial/JTAG, or RTT.

The crate documentation states the line grammar and how an application connects the console to its transport and commands. The requirements it meets are K-1 to K-26 in the [embedded-devkit requirements](../REQUIREMENTS.md).

## Commands for a library

A library's console commands go in their own crate, so the library does not depend on `mcu-console`. Name the crate `mcu-console-<library>`, such as `mcu-console-nats`, so that a search for `mcu-console` finds it. A name such as `<library>-console` does not appear in that search.

## Licence

MIT or Apache-2.0, at your option.
