// The board that the shared examples in `esp/examples/` build for. Each example includes this file.

use embedded_devkit_esp32_s31_function_coreboard_1 as board;

/// The name the examples give the console, which its banner and prompt show.
#[allow(dead_code, reason = "the examples without a console do not use it")]
const NAME: &str = "s31";

/// The length of each of `net iperf`'s socket buffers, receive and send. The receive buffer and the network
/// driver's burst size, such as the Ethernet transmit ring's length, cap the TCP window the board advertises. Each
/// board sets its own, for its RAM.
#[allow(dead_code, reason = "the examples without a network do not use it")]
const IPERF_SOCKET: usize = 65536;

/// The length of `net iperf`'s work buffer, which holds one TCP write or one UDP datagram.
#[allow(dead_code, reason = "the examples without a network do not use it")]
const IPERF_WORK: usize = 8192;
