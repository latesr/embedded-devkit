# ADR-0005: The minimal board crate

**Status: accepted**

Covers: the board crates and the family crate, `embedded-devkit-esp`.

## Context

Two kinds of application run on the devkit's boards: the test firmware of each library repo, and a starter platform that runs a network client, a vault, and an integrator's own application in one firmware. Each needs its board brought up before anything else runs. The chip HALs, esp-hal and embassy-stm32, give an application every peripheral of the chip. What a board adds to its chip is small: the console's transport, the PSRAM settings, an Ethernet PHY and its pins, and the crystal. The devkit serves each repo that syncs from it, and depends on none of them.

## Decision

- `init` returns the HAL's `Peripherals`, and the application drives each one directly with the HAL, because the HAL already gives an API for every peripheral of the chip. The board crate takes only what its own functions use, such as the console's transport and the flash that the vault writes.
- The board crate adds a capability only when every application on the board needs it in the same way, such as the console's transport, `boot_count`, `reset`, `info`, and the network stack. An LED or a button is the application's.
- A capability is a function, as for flash access and the network, because a function that the application does not call costs nothing in the image. A feature exists only for a heavy dependency, such as the radio blobs behind `wifi` or the executors behind `embassy`.
- A board crate implements the traits of ecosystem crates, such as the RustCrypto traits on the hardware AES, and of the devkit's own crates. It implements no trait of a repo that uses it, because the devkit depends on none of them. A library adapts the board's functions to its own traits in its test firmware.

## Rejected

- **A board API that wraps the HAL.** The board crate would repeat the HAL's API for each peripheral, and an application could use only what the board crate wraps.
- **A feature for each capability.** A feature without a heavy dependency saves nothing, and adds a build variant.
- **The board crates implement each library's traits.** The devkit would depend on the repos that it serves.

## Consequences

- An application brings up any peripheral that the board crate does not, from the `Peripherals` that `init` returns.
- A board crate's public API holds its HAL's types, so an application builds against the HAL revision that the board crate pins.
- The board crates are not on crates.io before 0.1.0, so an application depends on them by git.
