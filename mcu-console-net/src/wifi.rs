//! The radio's side of the `net wifi` commands.

use core::fmt::{self, Display, Formatter};

/// A Wi-Fi station that the `net wifi` commands drive. A board crate implements it over its radio driver.
///
/// The console is one caller of the trait. An application can also call [`Wifi::join`] directly, with a network
/// that it holds, such as a record in the vault.
///
/// `()` is a station with no radio, for a board on Ethernet. Every method on it fails with
/// [`WifiError::Radio`], and [`NetCommands`](crate::NetCommands) does not offer the `net wifi` commands for it.
pub trait Wifi {
    /// Lists the access points in range, calling `found` once for each, in any order.
    async fn scan(&mut self, found: &mut impl FnMut(AccessPoint)) -> Result<(), WifiError>;

    /// Joins `network`, and returns once associated. With a BSSID, joins only that access point. Leaves any
    /// network joined before.
    async fn join(&mut self, network: &Network<'_>) -> Result<(), WifiError>;

    /// Drops the link to the access point joined now, as a drop by the access point would, and keeps the network,
    /// so the station joins it again by itself. It lets a test check the rejoin (N-14). A station with a radio and
    /// nothing joined does nothing and succeeds.
    async fn drop_link(&mut self) -> Result<(), WifiError>;

    /// The access point joined now, with its current signal strength, or `None`.
    fn status(&mut self) -> Option<AccessPoint>;
}

impl Wifi for () {
    async fn scan(&mut self, _: &mut impl FnMut(AccessPoint)) -> Result<(), WifiError> {
        Err(WifiError::Radio)
    }

    async fn join(&mut self, _: &Network<'_>) -> Result<(), WifiError> {
        Err(WifiError::Radio)
    }

    async fn drop_link(&mut self) -> Result<(), WifiError> {
        Err(WifiError::Radio)
    }

    fn status(&mut self) -> Option<AccessPoint> {
        None
    }
}

/// Why a scan or a join failed. A board crate logs the radio's own reason with defmt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum WifiError {
    /// No access point answered for the SSID, or for the BSSID when one is set.
    NotFound,
    /// The access point refused the join. A wrong password most often shows as this.
    Rejected,
    /// The join started and did not finish.
    Failed,
    /// The radio driver failed before it could try.
    Radio,
}

impl WifiError {
    /// The reason the console writes after `err`.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            WifiError::NotFound => "no such network",
            WifiError::Rejected => "the network refused the join",
            WifiError::Failed => "the join failed",
            WifiError::Radio => "the radio failed",
        }
    }
}

/// An SSID: up to 32 bytes, which need not be UTF-8. A hidden network's SSID is empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ssid {
    len: u8,
    bytes: [u8; 32],
}

impl Ssid {
    /// The SSID `bytes`, or `None` when it is longer than 32 bytes.
    pub fn new(bytes: &[u8]) -> Option<Self> {
        let mut ssid = Ssid {
            len: u8::try_from(bytes.len()).ok()?,
            bytes: [0; 32],
        };
        ssid.bytes.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(ssid)
    }

    /// The SSID's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

impl Display for Ssid {
    /// Writes the SSID as text. A byte that is not part of valid UTF-8 is written as `\xNN`, and an empty SSID
    /// as `(hidden)`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.len == 0 {
            return f.write_str("(hidden)");
        }
        for chunk in self.as_bytes().utf8_chunks() {
            f.write_str(chunk.valid())?;
            for byte in chunk.invalid() {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

/// One access point, from a scan or from [`Wifi::status`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AccessPoint {
    /// The network's name.
    pub ssid: Ssid,
    /// The access point's MAC address.
    pub bssid: [u8; 6],
    /// The channel it is on.
    pub channel: u8,
    /// The signal strength in dBm.
    pub rssi: i8,
}

/// The network [`Wifi::join`] joins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Network<'a> {
    /// The network's name.
    pub ssid: &'a Ssid,
    /// The one access point to join, or `None` for any that broadcasts the SSID.
    pub bssid: Option<[u8; 6]>,
    /// The password, empty for an open network.
    pub password: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssid_holds_up_to_32_bytes_of_any_value() {
        let longest = [0xffu8; 32];
        assert_eq!(Ssid::new(&longest).unwrap().as_bytes(), longest);
        assert_eq!(Ssid::new(b"").unwrap().as_bytes(), b"");
        assert_eq!(Ssid::new(&[0u8; 33]), None);
    }
}
