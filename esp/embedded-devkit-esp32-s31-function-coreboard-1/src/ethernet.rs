//! Ethernet through the board's Motorcomm YT8531 PHY over RGMII, and an embassy-net stack over it (B-7, B-8, B-23).

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_net_driver::Capabilities;
use esp_hal::ethernet::{EthernetDmaStorage, RgmiiPinBundle};
use esp_hal::peripherals::{
    ETH, GPIO5, GPIO6, GPIO7, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, GPIO14, GPIO15, GPIO16,
    GPIO17, GPIO18, GPIO19,
};

pub use embedded_devkit_esp::ethernet::EthernetError;

/// The board's Ethernet pins: the RGMII clocks, controls and data, the PHY's management bus and its reset line.
pub struct Pins {
    /// The management bus clock.
    pub mdc: GPIO5<'static>,
    /// The management bus data.
    pub mdio: GPIO6<'static>,
    /// The PHY's reset, active low.
    pub phy_reset: GPIO7<'static>,
    /// Transmit data bit 0.
    pub txd0: GPIO8<'static>,
    /// Transmit data bit 1.
    pub txd1: GPIO9<'static>,
    /// Transmit data bit 2.
    pub txd2: GPIO10<'static>,
    /// Transmit data bit 3.
    pub txd3: GPIO11<'static>,
    /// Transmit control.
    pub tx_ctl: GPIO12<'static>,
    /// The transmit clock, which the chip drives.
    pub tx_clk: GPIO13<'static>,
    /// The receive clock, which the PHY drives.
    pub rx_clk: GPIO14<'static>,
    /// Receive control.
    pub rx_ctl: GPIO15<'static>,
    /// Receive data bit 3.
    pub rxd3: GPIO16<'static>,
    /// Receive data bit 2.
    pub rxd2: GPIO17<'static>,
    /// Receive data bit 1.
    pub rxd1: GPIO18<'static>,
    /// Receive data bit 0.
    pub rxd0: GPIO19<'static>,
}

/// Resets the PHY, starts the MAC and the embassy-net stack over it with DHCP, and spawns the task that runs them.
/// The MAC uses `rings`, `RX` receive and `TX` transmit descriptors, in internal RAM as a static is. Call it once,
/// after [`embassy::start`](crate::embassy::start).
///
/// Returns the stack and the driver's capabilities. `max_transmission_unit` is for `mcu-console-net`'s
/// `NetCommands::ethernet`. `max_burst_size` is `TX`, and caps the TCP window smoltcp advertises.
///
/// Fails with [`EthernetError::Phy`] when the MAC or the PHY does not start, or when the chip's base MAC cannot
/// be read. Fails with [`EthernetError::Spawn`] when the task cannot be spawned, or on a second call.
pub async fn start<const RX: usize, const TX: usize>(
    spawner: Spawner,
    eth: ETH<'static>,
    pins: Pins,
    rings: &'static mut EthernetDmaStorage<RX, TX>,
) -> Result<(Stack<'static>, Capabilities), EthernetError> {
    let bundle = RgmiiPinBundle {
        rx_clk: pins.rx_clk,
        tx_clk: pins.tx_clk,
        rx_ctl: pins.rx_ctl,
        tx_ctl: pins.tx_ctl,
        rxd0: pins.rxd0,
        rxd1: pins.rxd1,
        rxd2: pins.rxd2,
        rxd3: pins.rxd3,
        txd0: pins.txd0,
        txd1: pins.txd1,
        txd2: pins.txd2,
        txd3: pins.txd3,
        mdc: pins.mdc,
        mdio: pins.mdio,
    };
    embedded_devkit_esp::ethernet::start(spawner, eth, bundle, pins.phy_reset, rings).await
}
