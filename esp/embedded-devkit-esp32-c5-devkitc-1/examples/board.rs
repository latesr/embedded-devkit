// The board that the shared examples in `esp/examples/` build for. Each example includes this file.

use embedded_devkit_esp32_c5_devkitc_1 as board;

/// The name the examples give the console, which its banner and prompt show.
#[allow(dead_code, reason = "the examples without a console do not use it")]
const NAME: &str = "c5";

/// The length of each of `net iperf`'s socket buffers, receive and send. The receive buffer and the network
/// driver's burst size, such as the Wi-Fi burst size, cap the TCP window the board advertises. Each board sets its
/// own, for its RAM.
#[allow(dead_code, reason = "the examples without a network do not use it")]
const IPERF_SOCKET: usize = 32768;

/// The length of `net iperf`'s work buffer, which holds one TCP write or one UDP datagram.
#[allow(dead_code, reason = "the examples without a network do not use it")]
const IPERF_WORK: usize = 8192;

/// The chip's USB D+ and D- pins, taken from `$peripherals`, for the console transport.
#[allow(unused_macros, reason = "the examples without a console do not use it")]
macro_rules! usb_pins {
    ($peripherals:ident) => {
        ($peripherals.GPIO14, $peripherals.GPIO13)
    };
}
