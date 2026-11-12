# mcu-console-perf

Performance figures for [mcu-console](../mcu-console/README.md). `perf run <command>` runs one of the application's commands, then writes the time it took, the CPU cycles, the cycles per instruction, the main stack's deepest point, and the heap.

- The crate is `no_std`. It does not allocate or use `unsafe`.
- A board crate implements the `Platform` trait. Only a counter and its rate are required, and every other figure is optional.
- A board crate gives its `Platform` behind a `perf` feature. Only the application turns that feature on, never a library, so an application built without it is unchanged.

The crate documentation lists the figures. The requirements it meets are P-1 to P-7 in the [embedded-devkit requirements](../REQUIREMENTS.md).

## Licence

MIT or Apache-2.0, at your option.
