# embedded-devkit-esp32-s31-function-coreboard-1

The board crate for the ESP32-S31-Function-CoreBoard-1 v1.0.

## PSRAM at 250 MHz

The board crate runs its code from PSRAM at 200 MHz, the rating in the module's datasheet. At 200 MHz, Ethernet runs at 100 Mbit/s at most. To run the PSRAM at 250 MHz, which also lets Ethernet run at 1 Gbit/s, set this in the application's `.cargo/config.toml`:

```toml
[env]
ESP_HAL_CONFIG_PSRAM_FREQUENCY = "250MHz"
```

250 MHz runs the PSRAM beyond the module's rating. Test each board before you rely on it. The on-target test `psram_holds_a_pattern_after_the_code_region` writes and reads back every word of the PSRAM that the code does not use.

## If the board stops answering

Firmware that sleeps or stops its USB can take the board's USB-JTAG off the bus, so `mcu list` does not show the board. To recover:

1. Hold BOOT, press and release RESET, then release BOOT. The chip starts in download mode, and its USB-JTAG returns.
2. Flash the firmware, such as `cargo run --release --example console`.
3. Press RESET. A reset from the probe keeps the chip in download mode, so the button starts the new firmware.
