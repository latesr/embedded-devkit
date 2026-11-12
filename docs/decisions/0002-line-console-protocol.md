# ADR-0002: The line console protocol

**Status: accepted**

Covers: `mcu-console`, its core in `input`, its `blocking` and `asynch` drivers, and the image ID in the banner, from the board crates' `mcu-run.x` to `mcu-run`'s `Banner`.

## Context

A library's test firmware needs an interface that a host test uses to run commands and read the results, across resets and over time. A developer runs the same commands from a terminal, to repeat a failing test's steps by hand. One interface must serve both, over the serial port or the RTT channel of the board. A console and its host can come from different releases, and a host must know which build answers. `REQUIREMENTS.md` states the behaviour of the console, and this ADR states why the console has that shape.

## Decision

- Each chip family takes the image ID from what its image format already holds, and each board crate's `build.rs` documents where.
- Each driver owns its transport and the echo, so the property tests run on the core alone, with the input split at every point.
- `Commands` is one async trait. The blocking driver polls the same future with an empty waker, so an application writes each command once, and either driver runs it.
- A tuple composes command sets, so the board crate's commands and the application's commands do not depend on each other.
- A command watches for the request to stop through its `Cancel` handle. The request is not tied to the Ctrl-C byte, so another transport, such as a NATS console, can raise it.

## Rejected

- **A blocking trait and an async trait for commands.** An application would write each command twice, or choose one driver for every command.
- **The GNU build ID on every chip.** espflash writes only program data into an ESP32 image, so the note never reaches the chip.
- **A hash of the flash that the firmware makes at boot.** Each boot would take longer, and the host would need the image layout to make the same hash.

## Consequences

- An application on the NUCLEO-H7S3L8 board crate links with `--build-id=sha1` beside `-Tmcu-run.x`, or its link fails.
- An ESP32 image that espflash did not make, or that another tool changed after espflash, gives no image ID, or gives the ID of another ELF.
- Under the blocking driver, a command cannot wait on another task, because no other task runs while the driver polls. Ctrl-C cannot stop a command there, because the driver does not read while a command runs. The network commands run under the async driver.
- A reply holds text only. A command that returns bytes writes them as text.
- A test that needs the boot output reads it over RTT through `mcu-run`, because the console drops the output that no host reads.
- A command can be dropped at any await, so it keeps nothing at an await that a drop leaves in a bad state.
