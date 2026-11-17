# Requirements

embedded-devkit holds the tools and the board crates that test firmware on real boards. `mcu-console` runs a console on the device, `mcu-console-net` adds network commands to it, `mcu-console-perf` adds performance figures, and `mcu-run` flashes and drives the device from the host. One board crate per dev board brings the board up. A library repo or an application uses them to test its code on a board.

Each requirement has an ID, such as K-3 or B-10. Tests, plans, and commits cite the ID. An ID is never reused after its requirement is removed. The letter names the requirement's section:

- K, the console, `mcu-console`
- N, the network console, `mcu-console-net`
- P, performance, `mcu-console-perf`
- M, the runner, `mcu-run`
- B, the board crates
- H, the hardware tests and their harness, `mcu-scenario`

## Terms

- **Board crate.** A crate named `embedded-devkit-<board>` that initialises the HAL for one dev board, hands its peripherals to the application, and adds what every application on the board shares.
- **Application.** The binary built on a board crate: an integrator's product, an example, or a library repo's test firmware. It supplies its own commands and chooses the board crate's features. App is its short form.
- **Family crate.** A crate that holds the code the board crates of one chip family share, such as `embedded-devkit-esp`. A board crate turns on its chip's feature in it. The Board crates requirements apply to the family crate's code as they apply to the board crate's.
- **Test firmware.** A binary in a library repo, under `firmware/<board>/`, that runs that library's tests on one board.
- **On-target test.** A test that runs on the board under `embedded-test`, one test per reset.
- **Scenario test.** A host Rust test that drives running firmware through `mcu-run` and the console.
- **Hardware test.** An on-target test or a scenario test. Both need a board attached.
- **Result line.** The line that ends a console reply: `ok` or `err <reason>`.
- **Selector.** A probe-rs `VID:PID:serial` string that names one board.
- **RTT.** Real-Time Transfer: channels through ring buffers in the chip's RAM, which the probe reads and writes while the core runs. A control block in RAM tells the probe where the buffers are.
- **Flash-write modes.** Where the other code runs while the flash is erased or written: nowhere, because it stalls, or from SRAM, or from PSRAM.

## Goals

- A library repo runs its tests on each reference board with one command.
- Host tests drive firmware through a console and check its replies.
- One text protocol serves a person at a terminal and a host test, so a developer repeats a failing test's commands by hand. A binary channel for tests, beside a text console, would give a test other commands than a person.
- Every [device](README.md#a-device-partly-built) follows the same flow: deploy the image, reset, run.
- Board crates serve blocking code and Embassy.

## Out of scope for v1

- RTIC, FreeRTOS, or a C toolchain. Both test environments use Embassy and the chip's HAL.
- The Xtensa ESP32 chips.
- Over-the-air update and secure boot.
- Power cycling. v1 resets the chip.

## Crates

| Crate | On crates.io | Contents |
|---|---|---|
| `mcu-console` | Later | The device console: a sans-I/O core, and blocking and async drivers |
| `mcu-console-net` | Later | The network console commands, over an embassy-net stack and a `Wifi` trait |
| `mcu-console-perf` | Later | The performance commands, over a `Platform` trait that a board crate implements |
| `mcu-run` | Later | The host runner: a library and a thin binary |
| `mcu-scenario` | Later | The scenario tests' harness, over `mcu-run` |
| `embedded-devkit-esp` | No | The family crate for the ESP32 board crates |
| `embedded-devkit-esp32-c5-devkitc-1` | No | The board crate for the ESP32-C5-DevKitC-1 v1.2 |
| `embedded-devkit-esp32-s31-function-coreboard-1` | No | The board crate for the ESP32-S31-Function-CoreBoard-1 v1.0 |
| `embedded-devkit-nucleo-h7s3l8` | No | The board crate for the NUCLEO-H7S3L8 |
| `xtask` | No | The checks and other repo tasks |

The five `mcu-` crates go to crates.io once they are stable. Until then, and for the board crates always, other repos depend on these crates by git tag.

- Each `mcu-` crate is built so that it can move to its own repo. Each states its own `version` and `rust-version`, and has its own `README.md`.
- The board crates are public on GitHub and not on crates.io before 0.1.0. They serve the test firmware of the library repos, and they are the starter platform on which an integrator writes an application.
- The shared xtask code is a copy from embedded-conventions, made by `cargo xtask sync` there. No crate depends on it.

## Console: `mcu-console`

- **K-1** The core is `no_std`, does not allocate, and is sans-I/O. It takes bytes in and gives bytes out, and does not perform I/O.
- **K-2** A blocking driver runs the core over `embedded-io`.
- **K-3** An async driver runs the core over `embedded-io-async`.
- **K-4** The console runs over RTT or over a serial port. The transport is whatever the driver's `embedded-io` implementation reaches.
- **K-5** The console is lockstep. For each line the host sends, it writes the echo of the line, then zero or more body lines, then one result line, then the prompt. Every output line ends in CRLF. The prompt does not end in CRLF.
  - Rationale: A reply ends at its result line, not at the prompt, so the protocol does not need framing or a length field. The prompt does not end with a line end, so a host that reads lines does not see it arrive.
- **K-6** The result line is `ok` or `err <reason>`.
- **K-7** The prompt is `<name>> `, with the name the application gives. The default is `mcu> `.
- **K-8** A line ends at CR, LF, or CRLF. A tab counts as a space. Any other control byte is ignored.
- **K-9** When the console starts, it writes a banner, then the prompt. The banner is `<name> boot <n> protocol <v>`, then ` image <id>` when the application names its image (K-26), and starts with a line end. The application supplies the boot counter. `<v>` is the version of the console protocol, which rises only when a host must change to keep working.
  - Rationale: The protocol version is in the banner, not in a `version` command. A host would send such a command before it knows that it can read the reply, and would get no version after a reset that it did not make.
- **K-26** `<id>` is the image's ID, such as the SHA-256 of its ELF file or its GNU build ID, in lower-case hex. `mcu-console` reads a GNU build ID from the linker's note, and refuses a note of another owner or type, or one that ends early.
  - Rationale: A host knows which build answers without a record of what it flashed. The ID is made from the image's bytes, not from a version string, which two builds of one version share.
- **K-10** The application supplies its commands as a command set. A command receives its arguments, writes body lines, and returns its result.
- **K-11** The console has two built-in commands. `help` writes one body line per command, over every set. `reset` writes `ok`, flushes it, and then resets the chip through a function the application supplies.
- **K-12** An unknown command gives an `err` result line that names the command.
- **K-13** A line longer than the console's buffer gives an `err` result line. The console discards the rest of the line up to its end.
- **K-14** The console echoes each byte as it is typed. Backspace, `0x08` or `0x7f`, deletes the last byte. Ctrl-C, `0x03`, abandons the line being typed and writes the prompt.
- **K-15** An empty line gives the prompt and no result line.
- **K-16** A line that is not UTF-8 gives `err not utf-8`.
- **K-17** Arguments are words split on whitespace. A command can take one free-text value, last: the rest of the line, with its outer whitespace removed. There is no quoting.
- **K-18** Command sets compose. The first set that knows a name runs it. The board crate's set and the application's set combine this way.
- **K-19** A command's help entry can mark its last argument as secret. The console echoes each byte of a secret argument as `*`.
- **K-20** Every write has a timeout the application sets. After a write times out, the console drops its output until the next input byte arrives. A blocking flush is bounded by the transport alone.
  - Rationale: The console drops the output, and does not wait for the host. With writes that wait, a board without an open terminal would stop at its first reply.
- **K-21** A body line is cut at the console's reply length.
- **K-22** A driver returns when the transport reports end of input or a read error.
- **K-23** While a command runs, the async driver reads input. The first Ctrl-C asks the command to stop, and a command that watches for it writes its result line. A second Ctrl-C ends the command with `err interrupted`. Every other byte is dropped.
  - Rationale: The first Ctrl-C only asks, so a command that runs until stopped can write its summary. The second drops the command, because not every command watches for the request.
- **K-24** The blocking driver does not read input while a command runs.
- **K-25** `mcu-console` gives commands two parsers. `flag` splits a `--name=value` word into its name and value, and refuses any other form. `parse_size` reads a byte count with no suffix, `B`, or `KB`, where `KB` is 1,024 bytes. It refuses every other suffix, such as `k`, and an overflow.

## Network console: `mcu-console-net`

- **N-1** `mcu-console-net` is `no_std` and does not allocate. It gives `mcu-console` a command set over an embassy-net `Stack`. Each of its commands starts with `net`.
- **N-2** `net config` writes `medium`, `mac`, `link`, `address`, `gateway`, `dns`, and `mtu`, in that order. On Wi-Fi it then writes `ssid`, `bssid`, `channel`, and `rssi`. A value the device does not have is `none`. The application gives the medium and the MTU, because the stack does not report them.
- **N-3** `net dns <name>` writes one `address` line for each A record. An IPv4 address is written back without a query.
- **N-4** `net ping <host> [count]` sends `count` echo requests, 5 by default, 0.5 s apart, each waiting up to 1 s. It writes one line, `ping: sent <n> received <n> min <t> avg <t> max <t>`, with each time in milliseconds. Ctrl-C stops it after the request in flight, and it writes the same line for the requests it sent. It fails with `err no reply` when none returns.
- **N-5** `net latency <host[:port]>` opens 20 TCP connections to the port, 443 by default, 0.5 s apart, each waiting up to 3 s. It times each connect and then aborts the connection with a RST. It writes `address`, and `connect: sent 20 opened <n> refused <n> timeout <n> min <t> median <t> p90 <t> max <t>`. It fails with `err no connection opened` when none opened.
- **N-15** `net iperf <host[:port]>` runs one iperf 2 client test against a server, port 5001 by default, over embedded-iperf2. It takes `-u`, `-R`, `-t <s>` or `-n <bytes>`, `-l <len>`, `-b <rate>` with an optional `k`, `m`, or `g` suffix, and `-i <s>`, with iperf 2.2.2's defaults. Each interval writes `interval: <a>-<b> s <bytes> bytes <rate> Mbit/s` while the test runs, and the end writes `result:` in the same shape. A UDP receiver's line adds `lost <n>/<n> ooo <n> jitter <t> ms`. A forward UDP test then writes the server report as `server:` in the same shape, or `server: none`. An interval is dropped when 16 already wait for the console. The application supplies the buffers, and without them the command fails with `err no iperf buffers`. A TCP connect fails with `err the server refused the connection`, `err the connection timed out` after 5 s, or `err no route to the host`.
  - Rationale: The buffers set the TCP window, and each board has its own room for them. Buffers inside `mcu-console-net` would have one size in every application, whether it runs `net iperf` or not.
- **N-17** `net iperf` comes with `mcu-console-net`'s `iperf` feature. The feature is off by default. Without it, `net iperf` is not in `help`, `net iperf` gives the `net` usage, and `mcu-console-net` does not depend on embedded-iperf2.
  - Rationale: The client and its buffers do not belong in a production build, so the feature is off by default, not on.
- **N-16** `mcu_console_net::dhcp_config` gives the stack configuration for DHCP that every board crate uses. Its client sends a DISCOVER again 2 s after the last one, until a server answers.
- **N-6** `net wifi scan` writes one line for each access point, strongest first and numbered from 1: `<n>: <ssid>, <bssid>, channel <c>, <rssi> dBm`. A hidden network's SSID is `(hidden)`. The command set keeps the strongest 16 for `net wifi use`.
- **N-7** `net wifi use <n>` sets the SSID and the BSSID from row `n` of the last scan, and writes them back.
- **N-8** `net wifi ssid <ssid>` sets the SSID to the rest of the line, and clears the BSSID.
- **N-9** `net wifi bssid <aa:bb:cc:dd:ee:ff>` sets the BSSID, and `net wifi bssid any` clears it.
- **N-10** `net wifi password <password>` sets the password. Its argument is secret (K-19).
  - Rationale: The network name and the password are console arguments, not built into the image. A built-in password would be in the image, in the build's environment, and in each log of the build.
- **N-11** `net wifi join` joins the network that is set, then waits up to 30 s for an IPv4 address. It writes `ssid`, `bssid`, and `address`. When the last scan showed more than one BSSID for the SSID and none is set, it also writes `bssids: <n> for this ssid, see net wifi scan`. It fails with `err no ssid` when none is set, and with `err no address yet` when the wait ends.
- **N-14** `net wifi drop` drops the link to the access point joined now, and keeps the network, so the station joins it again without a command, as B-19 states. A test uses it to check the rejoin.
- **N-12** A board crate implements `mcu-console-net`'s `Wifi` trait over its radio. `scan` calls a function once for each access point. `join` joins a network, only its BSSID when one is given, and returns once associated. `drop_link` drops the link the way the access point dropping it would, and keeps the network. `status` gives the access point joined now.
- **N-13** `Wifi` fails with a non-exhaustive `WifiError`: `NotFound`, `Rejected`, `Failed`, or `Radio`. The console writes each as a fixed reason. The board crate logs the radio's own reason with defmt.

## Performance: `mcu-console-perf`

- **P-1** `mcu-console-perf` is `no_std`, does not allocate, and does not use `unsafe`. It gives `mcu-console` a command set that wraps the application's own. A line that does not start with `perf` goes to the application's commands unchanged.
- **P-2** `perf run <command> [args]` runs the command and writes its lines, then `perf_us`, `perf_cycles`, `perf_cpi`, `perf_stack`, `perf_heap`, and `perf_locks`, in that order. The result line is the command's.
- **P-3** `perf_us` is the command's time on the application's clock. `perf_cycles` is the board counter's count over that time. For a counter narrower than 64 bits, it is exact across wraps while the clock is off by less than half a wrap.
- **P-4** `perf_cpi` is the cycles per instruction retired in that time, to two decimals. It counts everything the core ran, and the cycles it spent waiting for an interrupt.
- **P-5** `perf_stack: size <b> deepest <b> headroom <b>` gives the main stack's size and its deepest point while the command ran. `perf_heap: used <b> peak <b> free <b>` gives the heap. A figure the board does not give is `none`.
- **P-6** A board crate gives the figures behind a `perf` feature, which only the application turns on. An application built without it is unchanged.
- **P-7** `perf_locks: longest_us <t> count <n> mean_us <t> at <a>` gives the locks held through the board crate's timed mutex while the command ran, to a tenth of a microsecond. `<a>` is the return address into the code that took the longest lock, in hex, or `none` when the board cannot give it. It does not count other locks, such as the HAL's, the RTOS's, or the radio's own critical sections. It is `none` when no such lock was taken.

## Runner: `mcu-run`

- **M-1** `mcu-run` is a library and a thin binary, `mcu`.
- **M-3** `mcu-run` finds a board by a device value. `probe://<text>` names the board with a probe-rs selector that contains `<text>`, ignoring case. It must match exactly one attached board. Otherwise `mcu-run` fails and lists the boards it found. Another `<scheme>://` value fails as not supported yet. A value without `://` names an entry in a devices file, and fails while there is none. With no value and one board attached, `mcu-run` uses that board.
- **M-19** `mcu run` and `mcu console` take the device value from `-d` (`--device`), or else from the `MCU_DEVICE` environment variable.
- **M-22** Without a device value and with more than one board attached, `mcu run` keeps each board with a probe that can debug the ELF chip's architecture. When more than one of those remains and the chip is not ARM, it keeps each board with a JTAG IDCODE that names the ELF's chip or names no chip. It uses the one board left, and otherwise fails as M-3 states with only the boards left. The chips keep running.
- **M-23** `mcu list` opens no probe. `mcu list -c` (`--chip`) opens each probe and shows the chip that its JTAG IDCODE names, or `?`. The chips keep running.
- **M-20** `Runner::console` returns a client on the image's console after the image starts, with the banner read. It first sends `reset` to the board's console. When the console answers `ok` within 2 s and the next banner within 10 s names the same image ID as the ELF (M-25), it keeps that console. When either ID is missing, they differ, or the console does not answer, it flashes the image. It writes one line to stderr, past a test harness's capture, that says whether it flashed the image or reset it. It fails when the banner names another protocol version than `mcu-run`'s, or none (K-9). `Runner::fresh_console` does the same, and always flashes the image.
  - Rationale: A host that misreads a reply fails far from the cause, so `mcu-run` refuses another protocol version, and does not only warn.
- **M-4** The console serial port of a board found by its probe is the port with the probe's USB serial number. On macOS it opens `/dev/cu.*`, never `/dev/tty.*`.
- **M-5** After a reset that drops the USB link, `mcu-run` waits for the same selector to return.
- **M-26** Before it flashes or resets a board, `mcu-run` fails when another process holds the board's console port, and names each such process with its PID. A host without `lsof` skips the check. `mcu list` shows the processes that hold each port.
  - Rationale: The firmware behaves differently while another program reads its USB console, and a flash resets the board under that program. `mcu-run` therefore fails. It does not warn and then flash.
- **M-6** `mcu-run` flashes the image, resets and halts the chip, blanks the RTT control block, and runs. It does the same on every board.
- **M-7** `mcu-run` loads a chip description from YAML at runtime, in place of the one built into probe-rs.
- **M-8** `mcu-run` shows the board's defmt output from RTT.
- **M-9** `mcu run` stops at a line that contains a given string, and exits 0. It stops at a deadline, and exits 124. When the firmware exits through semihosting, `mcu run` exits with the firmware's code. A failure of `mcu` exits 2.
- **M-10** `mcu-run` writes the board's output to a log file when asked.
- **M-18** A run follows the board across a reset, whether a software reset or the reset button. It writes `(the board reset)` and goes on with the next boot's output. When the reset leaves the core unreadable, `mcu-run` attaches to the chip again without resetting it. When the reset takes the probe off the USB bus, it waits for the probe to return as M-5 states, then attaches.
- **M-11** `mcu-run` runs `embedded-test` binaries and reports each test's result.
- **M-12** The library lets a host test send a console line and receive the body lines and the result line. The client works over any `Read + Write`. Each command has a timeout. The echo is the first line that ends with the command. After a reset, the client discards everything up to the banner and the first prompt. `Reply::field` gives the value of the first body line `<key>: <value>` for a key.
- **M-13** `mcu-run` takes the chip from the ELF's `.probe-rs.chip` section. `--chip` replaces it. Without either, `mcu-run` fails and names the ELF.
- **M-14** The image options are the partition table, the app partition, and the bootloader. The image format comes from the chip description.
- **M-24** `mcu-run` builds an ESP-IDF image in the flash mode that `--flash-mode` names, or else the ELF's `.mcu-run.flash-mode` section, or else QIO.
  - Rationale: A quad read moves twice the bits of a DIO read for each clock, and the app runs from flash through the cache. Most boards can run quad, so the default is not DIO, the default of ESP-IDF and espflash.
- **M-15** `--preverify` skips the pages that already hold the image's bytes. `--verify` reads the flash back after the write.
- **M-16** `mcu run` works as a cargo runner. Options may follow the ELF, where `cargo run -- <options>` puts them.
- **M-17** `mcu console` is interactive by default, with each keystroke sent to the board. Ctrl-] or Ctrl-D exits. On connecting it sends Ctrl-C, so the console writes a fresh prompt without running a line. When the port goes, as a reset button's reset takes it with the chip's USB, it waits for the port to return as M-5 states and connects again. With `--send <line>` it sends one line, prints the body and the result line, and exits 0 on `ok`, 1 on `err`, and 124 at the timeout. With `--interrupt-after <s>` as well, it sends Ctrl-C that many seconds after the line, and a second at twice that.
- **M-21** `mcu run --console` flashes the image, waits for its banner on the console port, and then runs the interactive console as M-17 states. A banner of another protocol version gives a warning, and the console still opens. It refuses `--until`, `--timeout`, and `--log`. For an embedded-test binary it fails with exit code 2 and names the ELF.
- **M-25** `mcu-run` reads the image ID from a console's banner, and from an ELF, each in the lower-case hex of K-26. An ELF with an ESP-IDF app descriptor has the SHA-256 of the file as its ID, and any other ELF has its GNU build ID. A banner with an image ID that is not whole bytes of lower-case hex is not a banner.

## Board crates

- **B-1** Each dev board has one board crate, named `embedded-devkit-<board>`.
- **B-2** A board crate brings up the chip, the clocks, time, and the console transport.
- **B-3** A board crate's core gives blocking handles and runs no executor. An `embassy` feature adds async drivers.
- **B-4** A board crate provides the executor setup.
- **B-6** A board crate or its HAL's build script provides the linker memory layout. The application copies only its `.cargo/config.toml`.
- **B-24** A board crate's firmware names its image in the console banner by the ID that `mcu-run` reads from the ELF (M-25). On the NUCLEO-H7S3L8 board crate, a link without the GNU build ID fails and says how to add it.
- **B-7** Flash access and the network are functions a test calls.
- **B-8** A board crate hands back every peripheral it does not use. It names the pins of standard connectors, such as the NUCLEO's Arduino connector.
- **B-9** A board crate implements the RustCrypto traits on the chip's hardware AES.
- **B-10** A board crate provides a handler for an `info` console command. It writes eight body lines, `key: value`, with the keys in this order: `board`, `chip`, `id`, `flash`, `psram`, `cpu`, `reset`, `firmware`.
- **B-11** `board` is the board crate's name without the `embedded-devkit-` prefix. `id` is the chip's MAC on the ESP32 boards, and the 96-bit unique device ID on the NUCLEO-H7S3L8. Sizes use the units the datasheets use, such as `8 MB`. A board without PSRAM writes `psram: none`.
- **B-12** `reset` is one of `power-on`, `software`, `watchdog`, `brown-out`, `pin`, `debugger`, or `other`. `debugger` is a reset that a debug probe made. The board crate maps its chip's reset flags onto this set. For a reason the set does not name, `other` is followed by the chip's own name for it in brackets, such as `other (CoreDeepSleep)`.
- **B-21** A board crate's boot count rises by one after a reset its own `reset` made, and restarts at 1 after any other reset.
  - Rationale: The count restarts so that the flash of a new image starts a new count. A count that survives every reset, such as one in flash, would make the first boot after a flash read as a reset.
- **B-13** A board crate supports flash writes in each flash-write mode its board allows. The integrator chooses the mode. No mode crashes.
- **B-14** The NUCLEO-H7S3L8's program lives in external flash and runs in place, or from SRAM, as the integrator chooses (B-13).
- **B-15** A board crate's network sends and receives IP packets of up to 1500 bytes.
- **B-19** After the link drops, a board crate's Wi-Fi joins the network it last joined again, without a command.
- **B-20** A board crate's network stack answers ICMP echo requests, so a host on the network can ping the board.
- **B-23** An application chooses the network driver's buffer sizes when it starts the network.
- **B-17** A board crate provides a function that computes a 32-byte pseudorandom function (PRF) of an input, under a root key in the chip that software cannot read. A library that needs a key source adapts this function to its own trait in its test firmware.
- **B-18** The console over RTT uses up channel 1 and down channel 0. defmt uses up channel 0. `mcu-run` defaults to the same channels.

An example of the `info` output on the ESP32-C5-DevKitC-1:

```
mcu> info
board: esp32-c5-devkitc-1
chip: esp32c5 v1.0
id: aa:bb:cc:dd:ee:01
flash: 8 MB
psram: 8 MB
cpu: 240 MHz
reset: software
firmware: my-app 0.1.0
ok
mcu>
```

## Hardware tests

- **H-1** Each board has two test environments. In the async environment, the library runs under Embassy. In the blocking environment, the library runs in a thread of its own, while the drivers run on Embassy executors.
- **H-2** On-target tests use `embedded-test`.
- **H-3** Scenario tests are host Rust tests that drive the firmware through the `mcu-run` library and the console.
- **H-4** Each flash-write mode is tested on at least one board, with the rest of the firmware running during the write.
- **H-5** A test resets the chip with the console's `reset` command, and checks `reset: software` in the `info` output after the reset.
- **H-6** Each board's `info` output names the board crate's board and CPU clock exactly, and the chip by name with its revision. Its ID equals the USB serial number of the board's probe on a board where that number is the chip's MAC, and otherwise has the form of the chip's unique ID. Its flash and PSRAM lines give a size, or `none` for PSRAM.
- **H-7** `mcu-scenario` builds a firmware's example or binary with cargo, in release, in the firmware's folder and with the firmware's own toolchain. It builds each firmware once per test process, and takes the ELF from cargo's messages. The build takes the test process's environment, and cargo's progress shows on the terminal while it builds. It fails with cargo's errors when the build fails, and names the target when cargo builds none of that name. An `mcu-scenario` error's debug text is its display text.
- **H-8** `mcu-scenario` takes the board from `MCU_DEVICE` as M-3 states, or else picks it from the firmware's chip as M-22 states.
- **H-9** In one test process, one test at a time holds the board, from the start of the firmware until its console is dropped. A test that panics while it holds the board leaves the board to the next test.
- **H-10** `Firmware::console` starts the firmware as M-20 states. `Firmware::fresh_console` always flashes it.
- **H-11** `env_or_skip` returns the value of an environment variable that is set and not empty. Otherwise it writes that the test is skipped, with the variable's name, to the process's stderr past the test harness's capture, and returns nothing. The test still reports `ok`.
- **H-13** `host_address` gives this host's IPv4 address on a board's subnet. It chooses a wired interface over Wi-Fi, and fails when no interface of the chosen medium is on the subnet, or when two are.
- **H-12** `Console::send_until` sends a line until a reply gives the value the test looks for, and returns that value. Each send waits at most 5 s, or the time left. It fails at the deadline with the line, the wait and the last reply, and at once when a send fails.

## Platforms

| Crate | Targets | MSRV |
|---|---|---|
| `mcu-console` | x86_64 and aarch64 hosts, `riscv32imac-unknown-none-elf`, `thumbv7em-none-eabihf`, `thumbv6m-none-eabi` | 1.91 |
| `mcu-console-net` | x86_64 and aarch64 hosts, `riscv32imac-unknown-none-elf`, `thumbv7em-none-eabihf`, `thumbv6m-none-eabi` | 1.91 |
| `mcu-console-perf` | x86_64 and aarch64 hosts, `riscv32imac-unknown-none-elf`, `thumbv7em-none-eabihf`, `thumbv6m-none-eabi` | 1.91 |
| `mcu-run` | x86_64 and aarch64 hosts, on macOS and Linux | 1.95 |
| `mcu-scenario` | x86_64 and aarch64 hosts, on macOS and Linux | 1.95 |
| `embedded-devkit-esp` | The target of each ESP32 board crate | 1.95 |
| `embedded-devkit-esp32-c5-devkitc-1` | `riscv32imac-unknown-none-elf` | 1.95 |
| `embedded-devkit-esp32-s31-function-coreboard-1` | `riscv32imafc-unknown-none-elf` | 1.95 |
| `embedded-devkit-nucleo-h7s3l8` | `thumbv7em-none-eabihf` | 1.91 |

A crate that goes into an application takes the lowest MSRV its dependencies allow. A host tool's MSRV does not limit any application's.

### Reference boards

- ESP32-C5-DevKitC-1 v1.2
- ESP32-S31-Function-CoreBoard-1 v1.0
- NUCLEO-H7S3L8

## Constraints

- Library code does not use `unsafe`. [ADR-0007](docs/decisions/0007-unsafe-in-board-crates.md) states the exceptions in a board crate.
- Library code never panics. It does not use `unwrap`, `expect`, `panic!`, or indexing that can go out of bounds. Arithmetic that can overflow uses a checked, saturating, or wrapping operation.
- `mcu-console` does not allocate memory, and does not use atomics.
- Each crate states its own MSRV, with edition 2024. The Platforms table gives the MSRV of each published crate and each board crate.
- Every host tool parses its arguments with clap.
- Every combination of a published crate's features builds.
- Traits from `embedded-io`, `embedded-io-async`, `embedded-hal`, `embedded-hal-async`, `defmt`, and the RustCrypto crates may appear in a public API. A board crate's public API also holds its HAL's types. `mcu-console-net`'s public API, and a board crate's network functions, also hold embassy-net types. No other dependency type appears in a public API without an ADR. `mcu-run`'s API holds no probe-rs type.

`EMBEDDED.md` names the lints that enforce the code constraints.

## Testing

Development is test first. `TESTING.md` states how a developer writes and checks tests. This section states what the test suite must contain.

### Unit and property tests

- The `mcu-console` core has property tests. Arbitrary input never panics, and splitting the input at any point gives the same output.
- `mcu-run`'s selector matching is tested on the host against lists of fake boards.
- `mcu-console-net`'s commands are tested on the host, over two embassy-net stacks wired together in one process. The far end answers pings, DNS queries, and TCP connects. A fake `Wifi` stands in for the radio.

### Hardware tests

The Hardware tests section states what runs on the boards. The hardware tests run on a developer's machine with the boards attached, before each release.

## Licence

Users may choose either the MIT licence or the Apache-2.0 licence. The Cargo `license` field is `MIT OR Apache-2.0`. Contributions are licensed on the same terms.
