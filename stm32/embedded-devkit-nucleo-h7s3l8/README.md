# embedded-devkit-nucleo-h7s3l8

The board crate for the NUCLEO-H7S3L8.

## If the board stops answering

The chip's supply configuration can be written once after each power-on. Firmware that writes a configuration other than the LDO's locks the chip in it, and the probe then cannot flash the board. The board crate's start-up code writes the LDO configuration, so its own firmware does not lock the chip. To recover from other firmware:

1. Erase the first 8 KB sector of the internal flash, so that the firmware does not start at the next power-on.
2. Turn the board off and on.
3. Flash the firmware again.
