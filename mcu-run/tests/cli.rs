//! The `mcu` command's options, run as a process. Each case fails before it needs a board.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;
use std::process::Command;

/// Writes an ELF that holds a `.probe-rs.chip` section with `chip`, or none, to a file of its own.
fn elf(name: &str, chip: Option<&str>) -> PathBuf {
    use object::write::Object;
    use object::{Architecture, BinaryFormat, Endianness, SectionKind};
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    if let Some(chip) = chip {
        let section = object.add_section(vec![], b".probe-rs.chip".to_vec(), SectionKind::Metadata);
        object.append_section_data(section, chip.as_bytes(), 1);
    }
    let path = std::env::temp_dir().join(format!("mcu-run-cli-{}-{name}.elf", std::process::id()));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

/// Runs `mcu` with `args`, and returns its exit code and its stderr.
fn mcu_run(args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_mcu"))
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn run_without_a_chip_names_the_elf_that_names_none() {
    let elf = elf("no-chip", None);
    let (code, stderr) = mcu_run(&["run", elf.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        format!(
            "mcu: {} names no chip in a .probe-rs.chip section. Pass --chip to name it.\n",
            elf.display()
        )
    );
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn the_chip_option_replaces_the_elf_s_chip() {
    for (name, chip) in [("with-chip", Some("esp32c5")), ("without-chip", None)] {
        let elf = elf(name, chip);
        let (code, stderr) = mcu_run(&["run", "--chip", "no-such-chip", elf.to_str().unwrap()]);
        assert_eq!(code, Some(2), "{name}");
        assert!(
            stderr.starts_with("mcu: unknown chip no-such-chip:"),
            "{name}: {stderr}"
        );
        std::fs::remove_file(elf).unwrap();
    }
}

#[test]
fn the_elf_s_chip_is_used_without_the_option() {
    let elf = elf("unknown-chip", Some("no-such-chip-in-elf"));
    let (code, stderr) = mcu_run(&["run", elf.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.starts_with("mcu: unknown chip no-such-chip-in-elf:"),
        "{stderr}"
    );
    std::fs::remove_file(elf).unwrap();
}

/// Runs `mcu` with `args` and `MCU_DEVICE` set to `device`, and returns its exit code and its stderr.
fn mcu_run_with_device(device: &str, args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_mcu"))
        .args(args)
        .env("MCU_DEVICE", device)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn run_and_console_take_the_device_from_mcu_device() {
    let elf = elf("device-from-env", Some("esp32c5"));
    for args in [&["run", elf.to_str().unwrap()][..], &["console"][..]] {
        let (code, stderr) = mcu_run_with_device("probe://no-such-board", args);
        assert_eq!(code, Some(2), "{args:?}");
        assert!(
            stderr.starts_with("mcu: no attached board matches probe://no-such-board."),
            "{args:?}: {stderr}"
        );
    }
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn the_device_option_and_its_short_form_replace_mcu_device() {
    let elf = elf("device-option", Some("esp32c5"));
    let path = elf.to_str().unwrap();
    for args in [
        &["run", "--device", "probe://no-such-option-board", path][..],
        &["run", "-d", "probe://no-such-option-board", path][..],
        &["console", "--device", "probe://no-such-option-board"][..],
        &["console", "-d", "probe://no-such-option-board"][..],
    ] {
        let (code, stderr) = mcu_run_with_device("probe://no-such-env-board", args);
        assert_eq!(code, Some(2), "{args:?}");
        assert!(
            stderr.starts_with("mcu: no attached board matches probe://no-such-option-board."),
            "{args:?}: {stderr}"
        );
    }
    std::fs::remove_file(elf).unwrap();
}

/// Writes an embedded-test binary for esp32c5, with the `.embedded_test` section and its version symbol, to a file
/// of its own.
fn test_binary(name: &str) -> PathBuf {
    use object::write::{Object, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let chip = object.add_section(vec![], b".probe-rs.chip".to_vec(), SectionKind::Metadata);
    object.append_section_data(chip, b"esp32c5", 1);
    let tests = object.add_section(vec![], b".embedded_test".to_vec(), SectionKind::Data);
    let at = object.append_section_data(tests, &1u32.to_le_bytes(), 4);
    object.add_symbol(Symbol {
        name: b"EMBEDDED_TEST_VERSION".to_vec(),
        value: at,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(tests),
        flags: SymbolFlags::None,
    });
    let path = std::env::temp_dir().join(format!("mcu-run-cli-{}-{name}.elf", std::process::id()));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

#[test]
fn run_with_the_console_refuses_an_embedded_test_binary() {
    let elf = test_binary("console-test-binary");
    let (code, stderr) = mcu_run(&["run", "--console", elf.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        format!(
            "mcu: {} is an embedded-test binary, which runs without a console\n",
            elf.display()
        )
    );
    std::fs::remove_file(elf).unwrap();
}
