//! Ethernet's start-up with descriptor rings the application chooses (B-23). The PHY starts without a cable, so the
//! test does not need a network.

#![no_std]
#![no_main]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use embedded_devkit_esp32_s31_function_coreboard_1 as _;

#[cfg(test)]
#[embedded_test::tests(executor = esp_rtos::embassy::Executor::new())]
mod tests {
    use embassy_executor::Spawner;
    use embedded_devkit_esp32_s31_function_coreboard_1::{embassy, ethernet, init};
    use esp_hal::ethernet::EthernetDmaStorage;
    use static_cell::ConstStaticCell;

    static RINGS: ConstStaticCell<EthernetDmaStorage<6, 7>> =
        ConstStaticCell::new(EthernetDmaStorage::new());

    /// The driver's burst size is the transmit ring's length, so the rings the application passes are the ones the
    /// driver uses.
    #[test]
    async fn the_burst_size_is_the_transmit_ring_the_firmware_chose() {
        let peripherals = init();
        embassy::start(peripherals.TIMG0);
        // SAFETY: embedded-test runs this body as a task on esp-rtos's Embassy executor, so the context is the
        // executor's own. embedded-test does not pass the test its spawner, and `SendSpawner` needs the
        // Ethernet task's arguments to be `Send`.
        #[expect(unsafe_code)]
        let spawner = unsafe { Spawner::for_current_executor() }.await;
        let pins = ethernet::Pins {
            mdc: peripherals.GPIO5,
            mdio: peripherals.GPIO6,
            phy_reset: peripherals.GPIO7,
            txd0: peripherals.GPIO8,
            txd1: peripherals.GPIO9,
            txd2: peripherals.GPIO10,
            txd3: peripherals.GPIO11,
            tx_ctl: peripherals.GPIO12,
            tx_clk: peripherals.GPIO13,
            rx_clk: peripherals.GPIO14,
            rx_ctl: peripherals.GPIO15,
            rxd3: peripherals.GPIO16,
            rxd2: peripherals.GPIO17,
            rxd1: peripherals.GPIO18,
            rxd0: peripherals.GPIO19,
        };
        let (_stack, capabilities) = ethernet::start(spawner, peripherals.ETH, pins, RINGS.take())
            .await
            .unwrap();
        defmt::assert_eq!(capabilities.max_burst_size, Some(7));
    }
}
