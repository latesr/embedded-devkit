# ADR-0008: The boot count

**Status: accepted**

Covers: `boot_count` and `reset` in the board crates and the family crate.

## Context

A scenario test resets the chip through the console, and expects the boot count in the next banner to rise by one ([ADR-0004](0004-scenario-tests-through-the-console.md)). The flash of a new image must not look like a software reset. A chip's reset reason may not tell a software reset from a probe's reset. RAM that survives a software reset holds random bytes after a power-on.

## Decision

- The count is in RAM that survives a software reset: RTC fast RAM on the ESP32 chips, and DTCM on the STM32H7S3. [ADR-0007](0007-unsafe-in-board-crates.md) covers the `unsafe` that its static needs.
- Where the reset reason cannot tell a software reset from a probe's, `reset` writes a marker beside the count, and the next boot counts a software reset only when the reason and the marker agree.

## Rejected

- **A backup register.** On the STM32H7S3, no safe API reaches one.
## Consequences

- A test that resets through the console sees the count rise by one. A test that resets through the probe sees 1.
- A new board needs RAM that survives a software reset. Where a probe can reset the board, it also needs a way to tell that reset from a probe's.
