//! The chip at a board's probe, found without stopping the chip.

use crate::{Board, Error};
use probe_rs::Architecture;
use probe_rs::config::Registry;
use probe_rs::probe::list::Lister;
use probe_rs::probe::{BitSequence, DebugProbeSelector, JtagBatch, WireProtocol};

/// Whether `board`'s probe can debug a chip of `architecture`. Opening the probe does not touch the chip. A probe
/// that does not open, such as one another process holds, cannot.
pub(crate) fn debugs(board: &Board, architecture: Architecture) -> bool {
    let selector = board.selector.clone();
    crate::probe_thread::on_probe_thread(move || {
        let Some(probe) = open(&selector) else {
            return false;
        };
        match architecture {
            Architecture::Arm => probe.has_arm_debug_interface(),
            Architecture::Riscv => probe.has_riscv_interface(),
            Architecture::Xtensa => probe.has_xtensa_interface(),
        }
    })
}

/// `board` with the chip that the JTAG IDCODEs at its probe name in `registry`, or with none.
pub(crate) fn with_chip(mut board: Board, registry: &Registry) -> Board {
    let selector = board.selector.clone();
    let idcodes = crate::probe_thread::on_probe_thread(move || idcodes(&selector));
    board.chip = chip_by_idcode(registry, &idcodes);
    board
}

fn open(selector: &str) -> Option<probe_rs::probe::Probe> {
    let selector: DebugProbeSelector = selector.parse().ok()?;
    Lister::new().open(selector).ok()
}

/// The IDCODE of each TAP on the JTAG chain at the probe, or none for a probe without RISC-V or Xtensa debug. A
/// chain scan and an IDCODE read reach the TAPs only, so the chip keeps running.
fn idcodes(selector: &str) -> Vec<u32> {
    let Some(mut probe) = open(selector) else {
        return Vec::new();
    };
    if !probe.has_riscv_interface() && !probe.has_xtensa_interface() {
        return Vec::new();
    }
    if probe.select_protocol(WireProtocol::Jtag).is_err() || probe.attach_to_unspecified().is_err()
    {
        return Vec::new();
    }
    let Some(mut chain) = probe.try_as_jtag_chain() else {
        return Vec::new();
    };
    let taps = chain.scan_chain().map_or(0, <[_]>::len);
    let mut found = Vec::new();
    for tap in 0..taps {
        if chain.select(tap).is_err() {
            continue;
        }
        // IR 1 selects IDCODE on every TAP that IEEE 1149.1 describes.
        let mut batch = JtagBatch::new();
        let ir = BitSequence::from_bytes(&1u32.to_le_bytes(), chain.params().irlen);
        chain.shift_ir(&mut batch, &ir);
        let handle = chain.exchange_dr(&mut batch, &BitSequence::repeat(false, 32));
        chain.run_test_idle(&mut batch, 0);
        let Ok(mut results) = chain.run(batch) else {
            continue;
        };
        let Ok(bits) = results.take(handle) else {
            continue;
        };
        found.push(word(bits.as_bits().iter().map(|bit| *bit)));
    }
    found
}

/// The 32-bit word that `bits` hold, least significant bit first, as a JTAG data register shifts it out.
fn word(bits: impl Iterator<Item = bool>) -> u32 {
    bits.take(32)
        .enumerate()
        .fold(0, |word, (at, bit)| word | (u32::from(bit) << at))
}

/// The chip that one of `idcodes` names in `registry`. Only a family that probe-rs identifies by its IDCODE
/// alone counts, because the magic word that tells other variants apart is read with the core halted.
fn chip_by_idcode(registry: &Registry, idcodes: &[u32]) -> Option<String> {
    registry.families().iter().find_map(|family| {
        let [variant] = family.variants.as_slice() else {
            return None;
        };
        family
            .chip_detection
            .iter()
            .filter_map(|method| method.as_espressif())
            .any(|info| info.variants.is_empty() && idcodes.contains(&info.idcode))
            .then(|| variant.name.clone())
    })
}

/// The board of `attached` to run an image for `chip`, of `architecture`, on. With `device`, or with fewer than
/// two boards, the board that [`select`](crate::select) finds. Otherwise the one board that `debugs` says can
/// debug the architecture and, when more than one can and the chip is not ARM, that `with_chip` names as `chip`
/// or as no chip. An ARM chip is not looked for, because probe-rs reads it only after an attach for a
/// known chip.
pub(crate) fn choose(
    attached: &[Board],
    device: Option<&str>,
    architecture: Architecture,
    chip: &str,
    mut debugs: impl FnMut(&Board, Architecture) -> bool,
    with_chip: impl FnMut(Board) -> Board,
) -> Result<Board, Error> {
    if device.is_some() || attached.len() < 2 {
        return crate::select(attached, device).cloned();
    }
    let debuggable: Vec<Board> = attached
        .iter()
        .filter(|board| debugs(board, architecture))
        .cloned()
        .collect();
    if debuggable.len() < 2 || architecture == Architecture::Arm {
        return one(debuggable, attached);
    }
    let named = debuggable.into_iter().map(with_chip).collect();
    one(by_chip(named, chip), attached)
}

/// Each board of `boards` with `chip`, ignoring case, or with no known chip.
pub(crate) fn by_chip(boards: Vec<Board>, chip: &str) -> Vec<Board> {
    boards
        .into_iter()
        .filter(|board| {
            board
                .chip
                .as_deref()
                .is_none_or(|found| found.eq_ignore_ascii_case(chip))
        })
        .collect()
}

/// The one board left in `left`. Fails with [`Error::NoBoard`], listing `attached`, when none is left, and with
/// [`Error::ManyBoards`] when more than one is.
pub(crate) fn one(mut left: Vec<Board>, attached: &[Board]) -> Result<Board, Error> {
    match left.len() {
        1 => Ok(left.remove(0)),
        0 => Err(Error::NoBoard {
            selector: None,
            boards: attached.to_vec(),
        }),
        _ => Err(Error::ManyBoards {
            selector: None,
            boards: left,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(selector: &str, chip: Option<&str>) -> Board {
        Board {
            selector: selector.to_owned(),
            name: "ESP JTAG".to_owned(),
            chip: chip.map(str::to_owned),
        }
    }

    #[test]
    fn an_idcode_names_the_espressif_chip_that_probe_rs_identifies_by_it_alone() {
        let registry = crate::chip::registry(None).unwrap();
        assert_eq!(
            chip_by_idcode(&registry, &[0x17C25, 0x17C25]).as_deref(),
            Some("esp32c5")
        );
        assert_eq!(
            chip_by_idcode(&registry, &[0x3C25, 0x20C25]).as_deref(),
            Some("esp32s31")
        );
        assert_eq!(
            chip_by_idcode(&registry, &[0x5C25]).as_deref(),
            Some("esp32c3")
        );
        // The Tensilica IDCODE that the ESP32, ESP32-S2 and ESP32-S3 share, an ARM debug port's IDCODE, and an
        // empty chain.
        assert_eq!(chip_by_idcode(&registry, &[0x1200_34E5]), None);
        assert_eq!(chip_by_idcode(&registry, &[0x6BA0_2477]), None);
        assert_eq!(chip_by_idcode(&registry, &[]), None);
    }

    /// Four boards: two ESP32s with probes that debug RISC-V, and two ST-LINKs that debug ARM.
    fn bench() -> Vec<Board> {
        vec![
            board("303a:1001:esp-a", None),
            board("303a:1001:esp-b", None),
            board("0483:3754:st-a", None),
            board("0483:3754:st-b", None),
        ]
    }

    fn debugs(board: &Board, architecture: Architecture) -> bool {
        match architecture {
            Architecture::Riscv => board.selector.starts_with("303a"),
            Architecture::Arm => board.selector.starts_with("0483"),
            Architecture::Xtensa => false,
        }
    }

    fn named(mut board: Board) -> Board {
        board.chip = if board.selector.ends_with("esp-a") {
            Some("esp32c5".to_owned())
        } else if board.selector.ends_with("esp-b") {
            Some("esp32s31".to_owned())
        } else {
            None
        };
        board
    }

    fn untouched(_: &Board, _: Architecture) -> bool {
        panic!("a probe was opened")
    }

    fn unnamed(_: Board) -> Board {
        panic!("a chip was looked for")
    }

    #[test]
    fn an_idcode_is_read_least_significant_bit_first() {
        let bits = |word: u32| (0..40).map(move |at| at < 32 && word >> at & 1 == 1);
        assert_eq!(word(bits(0x0001_7C25)), 0x0001_7C25);
        assert_eq!(word(bits(0x8000_0001)), 0x8000_0001);
        assert_eq!(word(std::iter::repeat(true)), u32::MAX);
        assert_eq!(word(std::iter::empty()), 0);
    }

    #[test]
    fn two_boards_are_chosen_between_by_their_probes() {
        let bench = bench();
        let two = vec![bench[0].clone(), bench[2].clone()];
        let chosen = choose(&two, None, Architecture::Riscv, "esp32c5", debugs, unnamed);
        assert_eq!(chosen, Ok(bench[0].clone()));
    }

    #[test]
    fn one_board_that_debugs_a_risc_v_chip_needs_no_chip_read() {
        let bench = bench();
        let one_esp = vec![bench[0].clone(), bench[2].clone(), bench[3].clone()];
        let chosen = choose(
            &one_esp,
            None,
            Architecture::Riscv,
            "esp32c5",
            debugs,
            unnamed,
        );
        assert_eq!(chosen, Ok(bench[0].clone()));
    }

    #[test]
    fn a_device_or_a_single_board_opens_no_probe() {
        let bench = bench();
        let chosen = choose(
            &bench,
            Some("probe://esp-b"),
            Architecture::Arm,
            "x",
            untouched,
            unnamed,
        );
        assert_eq!(chosen, Ok(bench[1].clone()));
        let one = vec![bench[2].clone()];
        let chosen = choose(
            &one,
            None,
            Architecture::Riscv,
            "esp32c5",
            untouched,
            unnamed,
        );
        assert_eq!(chosen, Ok(bench[2].clone()));
    }

    #[test]
    fn the_chip_picks_one_of_several_boards_that_debug_its_architecture() {
        let bench = bench();
        let chosen = choose(&bench, None, Architecture::Riscv, "esp32s31", debugs, named);
        assert_eq!(chosen, Ok(named(bench[1].clone())));
        let chosen = choose(&bench, None, Architecture::Riscv, "esp32c5", debugs, named);
        assert_eq!(chosen, Ok(named(bench[0].clone())));
    }

    #[test]
    fn the_architecture_alone_picks_a_board_without_a_chip_read() {
        let bench = bench();
        let one_st = vec![bench[0].clone(), bench[1].clone(), bench[2].clone()];
        let chosen = choose(
            &one_st,
            None,
            Architecture::Arm,
            "STM32H7S3L8",
            debugs,
            unnamed,
        );
        assert_eq!(chosen, Ok(bench[2].clone()));
    }

    #[test]
    fn several_arm_boards_are_listed_without_a_chip_read() {
        let bench = bench();
        let chosen = choose(
            &bench,
            None,
            Architecture::Arm,
            "STM32H7S3L8",
            debugs,
            unnamed,
        );
        assert_eq!(
            chosen,
            Err(Error::ManyBoards {
                selector: None,
                boards: bench[2..].to_vec(),
            })
        );
    }

    #[test]
    fn no_board_that_debugs_the_architecture_lists_every_board() {
        let bench = bench();
        let chosen = choose(
            &bench,
            None,
            Architecture::Xtensa,
            "esp32s3",
            debugs,
            unnamed,
        );
        assert_eq!(
            chosen,
            Err(Error::NoBoard {
                selector: None,
                boards: bench.clone(),
            })
        );
    }

    #[test]
    fn a_board_stays_when_its_chip_matches_or_is_not_known() {
        let boards = vec![
            board("c5", Some("esp32c5")),
            board("s31", Some("esp32s31")),
            board("unknown", None),
        ];
        let left: Vec<String> = by_chip(boards, "ESP32C5")
            .into_iter()
            .map(|board| board.selector)
            .collect();
        assert_eq!(left, ["c5", "unknown"]);
    }

    #[test]
    fn exactly_one_board_must_be_left() {
        let attached = [board("c5", Some("esp32c5")), board("s31", Some("esp32s31"))];
        assert_eq!(
            one(vec![attached[0].clone()], &attached),
            Ok(attached[0].clone())
        );
        assert_eq!(
            one(Vec::new(), &attached),
            Err(Error::NoBoard {
                selector: None,
                boards: attached.to_vec(),
            })
        );
        assert_eq!(
            one(attached.to_vec(), &[]),
            Err(Error::ManyBoards {
                selector: None,
                boards: attached.to_vec(),
            })
        );
    }
}
