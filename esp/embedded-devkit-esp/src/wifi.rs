//! Wi-Fi through esp-radio, an embassy-net stack over it, and `mcu-console-net`'s [`Wifi`] trait (B-7, B-15, B-19, B-23).

use alloc::vec::Vec;
use core::cell::Cell;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::{Runner, Stack, StackResources};
use embassy_net_driver::Driver as _;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer, WithTimeout};
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi::scan::{ScanConfig, ScanTypeConfig};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config, ConnectionError, DisconnectReason, Interface, Password,
    WifiController,
};
use mcu_console_net::{AccessPoint, Network, Ssid, Wifi, WifiError};
use static_cell::StaticCell;

/// The stack's sockets: DHCP and DNS, one each for `net ping` and `net latency`, and four for the application.
const SOCKETS: usize = 8;

/// How long the radio task waits before it joins again after a drop, or after a failed attempt to.
const REJOIN_DELAY: Duration = Duration::from_secs(2);

/// How often the radio task reads the signal strength while joined, for `net config`.
const STATUS_REFRESH: Duration = Duration::from_secs(2);

/// How long a scan stays on each channel. esp-radio's default active scan stays 10 to 20 ms, and missed access
/// points that do not answer a probe that fast. An access point sends a beacon every 102.4 ms by default, so a
/// longer stay hears every one in range.
const SCAN_DWELL: esp_hal::time::Duration = esp_hal::time::Duration::from_millis(120);

/// The longest the radio task spends on one join, leaving the network before included. It is shorter than
/// `mcu-console-net`'s own join timeout, so the task answers before `mcu-console-net` stops waiting.
const JOIN_LIMIT: Duration = Duration::from_secs(15);

/// What the station asks the radio task, numbered so that the station can tell its own answer from a late one.
struct Request {
    number: u32,
    ask: Ask,
}

enum Ask {
    Scan,
    Join(StationConfig),
    Drop,
}

/// The radio task's answer, with the number of the request it answers.
struct Response {
    number: u32,
    answer: Answer,
}

enum Answer {
    Scanned(Result<Vec<AccessPoint>, WifiError>),
    Joined(Result<(), WifiError>),
    Dropped,
}

static REQUESTS: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
static RESPONSES: Channel<CriticalSectionRawMutex, Response, 1> = Channel::new();
static STATUS: Mutex<CriticalSectionRawMutex, Cell<Option<AccessPoint>>> =
    Mutex::new(Cell::new(None));
static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// esp-radio's driver settings, which [`start`] takes: the receive and transmit queues, the driver's receive
/// buffers and the block-ack window, among others. Its setters are behind esp-radio's `unstable` feature, which
/// the board crate turns on.
pub use esp_radio::wifi::ControllerConfig;

/// Starts the radio as a station with `config`, and the embassy-net stack over it with DHCP. Spawns the tasks
/// that run both, and returns the stack and the [`Station`] that joins networks. Call it once, after
/// [`embassy::start`](crate::embassy::start).
///
/// esp-radio's burst size is not in `config`. The application sets it at build time with
/// `ESP_RADIO_CONFIG_WIFI_MAX_BURST_SIZE`.
///
/// `start` gives esp-radio a heap of 64 KiB of reclaimed RAM and 36 KiB of other internal RAM. An application that
/// raises esp-radio's buffer counts above the defaults adds internal RAM to the heap before it calls `start`,
/// with `esp_alloc::heap_allocator!`. The region must not be in the reclaimed segment, which `start` fills, or
/// the link fails. esp-alloc takes three regions by default, so the application adds one, and a fourth panics. An
/// application that needs more raises `ESP_ALLOC_CONFIG_MAX_REGIONS` in its `.cargo/config.toml`.
///
/// Fails with [`WifiError::Radio`] when the radio does not start with `config`, when a task cannot be spawned,
/// or on a second call.
pub fn start(
    spawner: Spawner,
    wifi: WIFI<'static>,
    config: ControllerConfig,
) -> Result<(Stack<'static>, Station), WifiError> {
    let resources = RESOURCES
        .try_init(StackResources::new())
        .ok_or(WifiError::Radio)?;
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);
    #[cfg_attr(not(feature = "esp32s31"), expect(unused_mut))]
    let mut controller = WifiController::new(wifi, config).map_err(|error| {
        defmt::error!("wifi: the radio did not start: {}", error);
        WifiError::Radio
    })?;
    #[cfg(feature = "esp32s31")]
    restart_driver(&mut controller)?;
    let interface = Interface::station();
    let mtu = interface.capabilities().max_transmission_unit;
    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let (stack, runner) =
        embassy_net::new(interface, mcu_console_net::dhcp_config(), resources, seed);
    spawner.spawn(net_task(runner).map_err(|_| WifiError::Radio)?);
    spawner.spawn(radio_task(controller).map_err(|_| WifiError::Radio)?);
    Ok((stack, Station { mtu, asked: 0 }))
}

/// Stops and starts the Wi-Fi driver once. `WifiController::new` sets the transmit power, and on the S31
/// that leaves the radio sending so slowly that an access point drops the station. `set_config` stops and starts
/// the driver when the mode changes, so a hidden access point's config and then the station's give the restart.
#[cfg(feature = "esp32s31")]
fn restart_driver(controller: &mut WifiController<'static>) -> Result<(), WifiError> {
    use esp_radio::wifi::ap::AccessPointConfig;
    let configs = [
        Config::AccessPoint(AccessPointConfig::default().with_ssid_hidden(true)),
        Config::Station(StationConfig::default()),
    ];
    configs.iter().try_for_each(|config| {
        controller.set_config(config).map_err(|error| {
            defmt::error!("wifi: the driver restart failed: {}", error);
            WifiError::Radio
        })
    })
}

/// The station, for `mcu-console-net`'s `net wifi` commands. A task owns the radio, and the station sends it each scan
/// and join. That task joins the last network again by itself after the link drops (B-19).
pub struct Station {
    mtu: usize,
    /// The number of the last request sent.
    asked: u32,
}

impl Station {
    /// The largest frame the driver takes, headers included, for `mcu-console-net`'s `net config`. It is 1514 when the
    /// application sets `ESP_RADIO_CONFIG_WIFI_MTU` as B-15 states.
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    /// Sends `ask` to the radio task and returns its answer. An answer to an earlier request, which a caller
    /// stopped waiting for, is dropped.
    async fn ask(&mut self, ask: Ask) -> Answer {
        self.asked = self.asked.wrapping_add(1);
        REQUESTS
            .send(Request {
                number: self.asked,
                ask,
            })
            .await;
        loop {
            let response = RESPONSES.receive().await;
            if response.number == self.asked {
                return response.answer;
            }
        }
    }
}

impl Wifi for Station {
    async fn scan(&mut self, found: &mut impl FnMut(AccessPoint)) -> Result<(), WifiError> {
        match self.ask(Ask::Scan).await {
            Answer::Scanned(result) => {
                result?.into_iter().for_each(found);
                Ok(())
            }
            _ => Err(WifiError::Radio),
        }
    }

    async fn join(&mut self, network: &Network<'_>) -> Result<(), WifiError> {
        match self.ask(Ask::Join(station_config(network)?)).await {
            Answer::Joined(result) => result,
            _ => Err(WifiError::Radio),
        }
    }

    async fn drop_link(&mut self) -> Result<(), WifiError> {
        match self.ask(Ask::Drop).await {
            Answer::Dropped => Ok(()),
            _ => Err(WifiError::Radio),
        }
    }

    fn status(&mut self) -> Option<AccessPoint> {
        STATUS.lock(Cell::get)
    }
}

/// esp-radio's station config for `network`: WPA2 or stronger with a password, open without one.
fn station_config(network: &Network<'_>) -> Result<StationConfig, WifiError> {
    let ssid = network
        .ssid
        .as_bytes()
        .try_into()
        .map_err(|_| WifiError::NotFound)?;
    let authentication = if network.password.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        let password = Password::try_from(network.password).map_err(|_| WifiError::Rejected)?;
        AuthenticationMethodConfig::Wpa2Personal(password)
    };
    let config = StationConfig::default()
        .with_ssid(ssid)
        .with_authentication(authentication);
    Ok(match network.bssid {
        Some(bssid) => config.with_bssid(bssid),
        None => config,
    })
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}

enum Event {
    Request(Request),
    Dropped,
    Rejoin,
    Refresh,
}

/// Owns the radio. Answers each request, keeps [`STATUS`] current, and joins the last network again after a
/// drop. Only a network that joined is kept, so a wrong password is not tried again and again.
#[embassy_executor::task]
async fn radio_task(mut controller: WifiController<'static>) -> ! {
    let mut network: Option<StationConfig> = None;
    let mut connected = false;
    loop {
        let event = if connected {
            match select3(
                REQUESTS.receive(),
                controller.wait_for_disconnect_async(),
                Timer::after(STATUS_REFRESH),
            )
            .await
            {
                Either3::First(request) => Event::Request(request),
                Either3::Second(_) => Event::Dropped,
                Either3::Third(()) => Event::Refresh,
            }
        } else if network.is_some() {
            match select(REQUESTS.receive(), Timer::after(REJOIN_DELAY)).await {
                Either::First(request) => Event::Request(request),
                Either::Second(()) => Event::Rejoin,
            }
        } else {
            Event::Request(REQUESTS.receive().await)
        };
        match event {
            Event::Request(Request {
                number,
                ask: Ask::Scan,
            }) => {
                let answer = Answer::Scanned(scan(&mut controller).await);
                RESPONSES.send(Response { number, answer }).await;
            }
            Event::Request(Request {
                number,
                ask: Ask::Join(config),
            }) => {
                let result = async {
                    if connected {
                        let _ = controller.disconnect_async().await;
                        STATUS.lock(|status| status.set(None));
                    }
                    connect(&mut controller, &config).await
                }
                .with_timeout(JOIN_LIMIT)
                .await
                .unwrap_or(Err(WifiError::Failed));
                connected = result.is_ok();
                network = connected.then_some(config);
                let answer = Answer::Joined(result);
                RESPONSES.send(Response { number, answer }).await;
            }
            Event::Request(Request {
                number,
                ask: Ask::Drop,
            }) => {
                // esp-radio's disconnect waits for the disconnect event itself, so the task cannot also see it in
                // wait_for_disconnect_async. The drop goes through the same handling as a drop it sees.
                if connected {
                    let _ = controller.disconnect_async().with_timeout(JOIN_LIMIT).await;
                    lost_link(&mut connected);
                }
                let answer = Answer::Dropped;
                RESPONSES.send(Response { number, answer }).await;
            }
            Event::Dropped => lost_link(&mut connected),
            Event::Rejoin => {
                if let Some(config) = &network {
                    connected = connect(&mut controller, config).await.is_ok();
                }
            }
            Event::Refresh => {
                if let Ok(rssi) = controller.rssi() {
                    STATUS.lock(|status| {
                        status.set(status.get().map(|joined| AccessPoint {
                            rssi: signal(rssi),
                            ..joined
                        }))
                    });
                }
            }
        }
    }
}

/// What the task does when the link drops: it marks itself not connected and clears [`STATUS`], and keeps the
/// network, so the next turn of its loop joins it again after [`REJOIN_DELAY`] (B-19).
fn lost_link(connected: &mut bool) {
    defmt::warn!("wifi: the link dropped, joining again");
    *connected = false;
    STATUS.lock(|status| status.set(None));
}

async fn scan(controller: &mut WifiController<'static>) -> Result<Vec<AccessPoint>, WifiError> {
    let config =
        ScanConfig::default()
            .with_show_hidden(true)
            .with_scan_type(ScanTypeConfig::Active {
                min: SCAN_DWELL,
                max: SCAN_DWELL,
            });
    let found = controller.scan_async(&config).await.map_err(|error| {
        defmt::warn!("wifi: the scan failed: {}", error);
        WifiError::Radio
    })?;
    Ok(found
        .iter()
        .filter_map(|access_point| {
            Some(AccessPoint {
                ssid: Ssid::new(access_point.ssid.as_bytes())?,
                bssid: access_point.bssid,
                channel: access_point.channel,
                rssi: access_point.signal_strength,
            })
        })
        .collect())
}

/// Joins `config` and records the access point in [`STATUS`]. Logs the radio's own reason for a failure.
async fn connect(
    controller: &mut WifiController<'static>,
    config: &StationConfig,
) -> Result<(), WifiError> {
    controller
        .set_config(&Config::Station(config.clone()))
        .map_err(|error| {
            defmt::warn!("wifi: the station config was refused: {}", error);
            WifiError::Radio
        })?;
    let connected = match controller.connect_async().with_timeout(JOIN_LIMIT).await {
        Ok(connected) => connected,
        Err(_) => {
            defmt::warn!("wifi: the join took longer than {} s", JOIN_LIMIT.as_secs());
            return Err(WifiError::Failed);
        }
    };
    match connected {
        Ok(info) => {
            let rssi = controller.rssi().map_or(0, signal);
            defmt::info!(
                "wifi: joined {} on channel {}",
                info.ssid.as_str(),
                info.channel
            );
            STATUS.lock(|status| {
                status.set(Ssid::new(info.ssid.as_bytes()).map(|ssid| AccessPoint {
                    ssid,
                    bssid: info.bssid,
                    channel: info.channel,
                    rssi,
                }))
            });
            Ok(())
        }
        Err(ConnectionError::Failed(info)) => {
            defmt::warn!("wifi: the join failed: {}", info.reason);
            Err(join_error(info.reason))
        }
        Err(ConnectionError::WifiError(error)) => {
            defmt::warn!("wifi: the radio failed: {}", error);
            Err(WifiError::Radio)
        }
        Err(_) => {
            defmt::warn!("wifi: the join failed for a reason this build does not name");
            Err(WifiError::Radio)
        }
    }
}

/// The [`WifiError`] for the reason a join failed (N-13).
fn join_error(reason: DisconnectReason) -> WifiError {
    match reason {
        DisconnectReason::NoAccessPointFound
        | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
        | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
        | DisconnectReason::NoAccessPointFoundInRssiThreshold => WifiError::NotFound,
        DisconnectReason::AuthenticationFailed
        | DisconnectReason::FourWayHandshakeTimeout
        | DisconnectReason::HandshakeTimeout
        | DisconnectReason::MicFailure => WifiError::Rejected,
        _ => WifiError::Failed,
    }
}

/// A signal strength in dBm, held to the range of an `i8`.
fn signal(rssi: i32) -> i8 {
    i8::try_from(rssi).unwrap_or(if rssi < 0 { i8::MIN } else { i8::MAX })
}
