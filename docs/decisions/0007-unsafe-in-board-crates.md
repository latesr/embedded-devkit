# ADR-0007: `unsafe` in the board crates

**Status: accepted**

Covers: the `unsafe` sites in the board crates and the family crate, and the lints they need.

## Context

`EMBEDDED.md` keeps `unsafe` out of library code with `unsafe_code = "forbid"`. A board crate sits between a HAL and a chip. Some things that every application needs have no safe API in the HALs that the devkit pins, such as RAM that survives a reset, an interrupt executor's handler, and code in a named section. `mcu-console-perf` does not use `unsafe`, so the figures that it reports, such as the deepest point of the stack, come from the board crate. A macro can expand to `unsafe` in the crate that invokes it, and the `unsafe_code` lint of that crate does not fire on it.

## Decision

- A board crate has an `unsafe` block only where the HAL gives no safe way to do what every application needs, or to read a figure that its `perf` feature reports. Each block has a `// SAFETY:` comment that states the invariant, and names the API that the HAL lacks.
- The board crates set `unsafe_code = "deny"`, and mark each site with `#[expect(unsafe_code)]` or `#[allow(unsafe_code)]`, because a forbid cannot be allowed at a site. A stray block then fails the build, and so does an `expect` on a site that no longer needs it. A search for `unsafe_code` finds each site that the lint sees.
- When one call in the HAL would remove a site, the devkit patches the HAL and submits the change, and the board crate does not read the register.
- A board crate's macro expands to `unsafe` only for the interrupt handler of an executor setup. Review checks each macro that a board crate exports, and each HAL macro that it invokes, because no lint sees what a macro expands to.

## Rejected

- **A register read in the board crate.** The crate would hold the driver's knowledge of the registers, and the block would stay for good.
- **`forbid(unsafe_code)` in the board crates.** A site could not allow it.
- **`unsafe` in `mcu-console-perf` for the figures.** The library crates would lose the rule that they forbid `unsafe`, and `mcu-console-perf` would hold the stack layout of each chip.

## Consequences

- A new site needs a `// SAFETY:` comment that names the missing HAL API.
- A HAL release that adds a safe API removes a site.
- A reviewer reads the expansion of each macro, because no check does.
