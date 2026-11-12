//! Ethernet through a PHY that esp-hal's generic driver runs, and an embassy-net stack over it (B-7, B-8, B-23).

use core::future::Future;
use core::pin::Pin;
use core::task::Context;
use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_net_driver::{Capabilities, Driver as _};
use embassy_time::{Duration, Timer};
use esp_hal::Async;
use esp_hal::ethernet::mac::{Duplex, LinkState, Speed};
use esp_hal::ethernet::phy::generic::GenericPhy;
use esp_hal::ethernet::phy::{MdioBus, Phy, PhyError};
use esp_hal::ethernet::{Ethernet, EthernetDmaStorage, EthernetPinBundle};
use esp_hal::gpio::{Level, Output, OutputConfig, OutputPin};
use esp_hal::peripherals::ETH;
use esp_hal::rng::Rng;
use static_cell::StaticCell;

/// The stack's sockets: DHCP and DNS, one each for `net ping` and `net latency`, and four for the firmware.
const SOCKETS: usize = 8;

/// How often the PHY's link state is read.
const LINK_POLL: Duration = Duration::from_millis(500);

/// Why Ethernet did not start.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum EthernetError {
    /// The MAC or the PHY did not come up, or the chip's base MAC could not be read.
    Phy,
    /// A task could not be spawned, or Ethernet had already started.
    Spawn,
}

type Driver = Ethernet<'static, Async, PolledPhy>;

static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// Resets the PHY, starts the MAC and the embassy-net stack over it with DHCP, and spawns the task that runs them.
/// `pins` are the board's MAC and management bus pins, such as esp-hal's `RgmiiPinBundle`. `phy_reset` is the
/// PHY's reset line, active low. The MAC uses `rings`, `RX` receive and `TX` transmit descriptors, in internal RAM
/// as a static is. Call it once, after [`embassy::start`](crate::embassy::start).
///
/// Returns the stack and the driver's capabilities. `max_transmission_unit` is for `mcu-console-net`'s
/// `NetCommands::ethernet`. `max_burst_size` is `TX`, and caps the TCP window smoltcp advertises.
///
/// Fails with [`EthernetError::Phy`] when the MAC or the PHY does not start, or when the chip's base MAC cannot
/// be read. Fails with [`EthernetError::Spawn`] when the task cannot be spawned, or on a second call.
pub async fn start<const RX: usize, const TX: usize>(
    spawner: Spawner,
    eth: ETH<'static>,
    pins: impl EthernetPinBundle + 'static,
    phy_reset: impl OutputPin + 'static,
    rings: &'static mut EthernetDmaStorage<RX, TX>,
) -> Result<(Stack<'static>, Capabilities), EthernetError> {
    let resources = RESOURCES
        .try_init(StackResources::new())
        .ok_or(EthernetError::Spawn)?;
    // esp-hal's Ethernet example holds the reset for 100 ms and waits 300 ms after its release, before the PHY
    // answers on the management bus.
    let mut phy_reset = Output::new(phy_reset, Level::Low, OutputConfig::default());
    Timer::after_millis(100).await;
    phy_reset.set_high();
    Timer::after_millis(300).await;
    let mac = mac().ok_or_else(|| {
        defmt::error!("ethernet: the chip's base MAC is not six bytes");
        EthernetError::Phy
    })?;
    let driver: Driver = Ethernet::new(eth, rings, mac, PolledPhy::new(), pins)
        .map_err(|error| {
            defmt::error!(
                "ethernet: the MAC or the PHY did not start: {}",
                defmt::Debug2Format(&error)
            );
            EthernetError::Phy
        })?
        .into_async();
    let capabilities = driver.capabilities();
    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(driver, mcu_console_net::dhcp_config(), resources, seed);
    spawner.spawn(net_task(runner, phy_reset).map_err(|_| EthernetError::Spawn)?);
    Ok((stack, capabilities))
}

/// The Ethernet MAC: the second of the chip's two universal addresses, the base MAC plus 1. The Wi-Fi station
/// takes the base, so both network interfaces have universal addresses. ESP-IDF gives that second address to
/// Bluetooth instead, and the devkit's BLE takes a static random address.
/// Returns `None` when esp-hal's base MAC is not six bytes.
fn mac() -> Option<[u8; 6]> {
    let base = esp_hal::efuse::base_mac_address();
    let &[a, b, c, d, e, f] = base.as_bytes() else {
        return None;
    };
    // One 48-bit increment, so a base ending in `ff` carries.
    let next = u64::from_be_bytes([0, 0, a, b, c, d, e, f]).wrapping_add(1);
    let [_, _, a, b, c, d, e, f] = next.to_be_bytes();
    Some([a, b, c, d, e, f])
}

/// Runs the stack, and holds the PHY's reset line high for as long as it runs. At the pinned esp-hal a dropped
/// `Output` keeps driving its pin. Holding it here keeps the PHY out of reset if a later esp-hal releases the pin
/// on drop.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Driver>, _phy_reset: Output<'static>) -> ! {
    runner.run().await
}

/// esp-hal's generic PHY driver, read every [`LINK_POLL`]. The generic driver wakes its task again at once on
/// every poll, which keeps the management bus busy.
struct PolledPhy {
    phy: GenericPhy,
    next: Timer,
    link: LinkState,
}

impl PolledPhy {
    fn new() -> Self {
        Self {
            phy: GenericPhy::new_auto(),
            next: Timer::after_ticks(0),
            link: LinkState {
                up: false,
                speed: Speed::_100M,
                duplex: Duplex::Full,
            },
        }
    }
}

impl Phy for PolledPhy {
    fn init<M: MdioBus>(&mut self, mdio: &mut M, max_speed: Speed) -> Result<(), PhyError> {
        self.phy.init(mdio, max_speed)
    }

    fn poll_link<M: MdioBus>(&mut self, mdio: &mut M, cx: Option<&mut Context<'_>>) -> LinkState {
        if let Some(cx) = cx {
            if Pin::new(&mut self.next).poll(cx).is_pending() {
                return self.link;
            }
            self.next = Timer::after(LINK_POLL);
            let _ = Pin::new(&mut self.next).poll(cx);
        }
        let link = self.phy.poll_link(mdio, None);
        if link.up != self.link.up || link.speed != self.link.speed {
            defmt::info!(
                "ethernet: link {}, {}",
                if link.up { "up" } else { "down" },
                defmt::Debug2Format(&link.speed)
            );
        }
        self.link = link;
        link
    }

    fn address(&self) -> u8 {
        self.phy.address()
    }
}
