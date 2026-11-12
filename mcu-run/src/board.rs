//! Finding the one board a run is for.

use crate::Error;
use probe_rs::probe::list::Lister;
use std::time::{Duration, Instant};

/// One attached debug probe, and so one board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    /// The probe-rs selector, `VID:PID:serial`: the VID and PID in lower-case hex, then the serial number as
    /// the probe reports it.
    pub selector: String,
    /// The probe's name, as probe-rs reports it.
    pub name: String,
    /// The chip at the probe, as probe-rs spells it, or `None` when it was not looked for or not found. Only
    /// [`attached_with_chips`] and a run's choice of board look for it.
    pub chip: Option<String>,
}

/// The boards attached now, each with the chip at its probe where the chip's JTAG IDCODE names it. Each probe is
/// opened, so one that another process holds gets no chip. The chips keep running.
pub fn attached_with_chips() -> Vec<Board> {
    let Ok(registry) = crate::chip::registry(None) else {
        return attached();
    };
    attached()
        .into_iter()
        .map(|board| crate::detect::with_chip(board, &registry))
        .collect()
}

/// The boards attached now. Each comes from the USB list alone, so no probe is opened.
pub fn attached() -> Vec<Board> {
    crate::register_espressif();
    crate::probe_thread::on_probe_thread(|| Lister::new().list_all())
        .into_iter()
        .map(|probe| Board {
            selector: format!(
                "{:04x}:{:04x}:{}",
                probe.vendor_id,
                probe.product_id,
                probe.serial_number.as_deref().unwrap_or("")
            ),
            name: probe.identifier,
            chip: None,
        })
        .collect()
}

/// The one board that `device` names. `probe://<text>` names the board with a probe-rs selector that contains
/// `<text>`, ignoring case.
///
/// With no device, the one attached board. Fails with [`Error::UnsupportedDevice`] for any other `<scheme>://`,
/// and with [`Error::NoSuchDevice`] for a value without one, which names an entry in a devices file. Fails with
/// [`Error::NoBoard`] when nothing matches and with [`Error::ManyBoards`] when more than one board does.
pub fn select<'a>(boards: &'a [Board], device: Option<&str>) -> Result<&'a Board, Error> {
    let wanted = device.map(probe_text).transpose()?.map(str::to_lowercase);
    let matching: Vec<&Board> = boards
        .iter()
        .filter(|board| {
            wanted
                .as_ref()
                .is_none_or(|wanted| board.selector.to_lowercase().contains(wanted))
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::NoBoard {
            selector: device.map(str::to_owned),
            boards: boards.to_vec(),
        }),
        many => Err(Error::ManyBoards {
            selector: device.map(str::to_owned),
            boards: many.iter().map(|board| (*board).clone()).collect(),
        }),
    }
}

/// The text after `probe://` in `device`.
fn probe_text(device: &str) -> Result<&str, Error> {
    match device.strip_prefix("probe://") {
        Some(text) => Ok(text),
        None if device.contains("://") => Err(Error::UnsupportedDevice {
            device: device.to_owned(),
        }),
        None => Err(Error::NoSuchDevice {
            device: device.to_owned(),
        }),
    }
}

/// How often a gone board is looked for.
const POLL: Duration = Duration::from_millis(250);

/// Whether `board` is attached now.
pub(crate) fn is_attached(board: &Board) -> bool {
    attached()
        .iter()
        .any(|found| found.selector == board.selector)
}

/// Waits up to `timeout` for `board` to be attached again, after a reset took its probe off the USB bus.
///
/// Fails with [`Error::Probe`] when it has not returned by the deadline.
pub(crate) fn wait_for_board(board: &Board, timeout: Duration) -> Result<(), Error> {
    let start = Instant::now();
    wait(
        board,
        timeout,
        attached,
        || start.elapsed(),
        || std::thread::sleep(POLL),
    )
}

fn wait<L, E, S>(
    board: &Board,
    timeout: Duration,
    mut list: L,
    mut elapsed: E,
    mut sleep: S,
) -> Result<(), Error>
where
    L: FnMut() -> Vec<Board>,
    E: FnMut() -> Duration,
    S: FnMut(),
{
    loop {
        if list().iter().any(|found| found.selector == board.selector) {
            return Ok(());
        }
        if elapsed() >= timeout {
            return Err(Error::Probe(format!(
                "{} did not return within {timeout:?}",
                board.selector
            )));
        }
        sleep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(selector: &str) -> Board {
        Board {
            selector: selector.to_owned(),
            name: "probe".to_owned(),
            chip: None,
        }
    }

    #[test]
    fn the_wait_ends_when_the_same_board_returns() {
        let wanted = board("303a:1001:D0");
        let mut lists = vec![
            vec![],
            vec![board("303a:1001:AA")],
            vec![board("303a:1001:D0")],
        ]
        .into_iter();
        let mut sleeps = 0;
        let waited = wait(
            &wanted,
            Duration::from_secs(10),
            || lists.next().unwrap_or_default(),
            || Duration::ZERO,
            || sleeps += 1,
        );
        assert_eq!(waited, Ok(()));
        assert_eq!(sleeps, 2);
    }

    #[test]
    fn the_wait_fails_once_the_timeout_passes() {
        let wanted = board("303a:1001:D0");
        let mut now = Duration::ZERO;
        let waited = wait(
            &wanted,
            Duration::from_secs(1),
            Vec::new,
            || {
                now += Duration::from_millis(400);
                now
            },
            || {},
        );
        assert_eq!(
            waited,
            Err(Error::Probe(
                "303a:1001:D0 did not return within 1s".to_owned()
            ))
        );
    }
}
