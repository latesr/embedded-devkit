//! Resolving an image's chip, with and without a chip description loaded at runtime (M-7, M-13), and its preferred
//! flash algorithm.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use mcu_run::{Error, FlashMode, Image, Runner};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn image(chip: &str, description: Option<&str>) -> Image {
    Image {
        elf: PathBuf::from("firmware.elf"),
        chip: chip.to_owned(),
        chip_description: description.map(fixture),
        ..Image::default()
    }
}

/// Writes an ELF with a non-alloc section for each of `sections`, as the board crates' `mcu-run.x` gives one,
/// to a file of its own, and returns the file's path.
fn elf_with(name: &str, sections: &[(&str, &[u8])]) -> PathBuf {
    elf_with_kinds(
        name,
        &sections
            .iter()
            .map(|(name, data)| (*name, *data, object::SectionKind::Metadata))
            .collect::<Vec<_>>(),
    )
}

/// Writes an ELF with a section of its kind for each of `sections`, to a file of its own, and returns the file's
/// path.
fn elf_with_kinds(name: &str, sections: &[(&str, &[u8], object::SectionKind)]) -> PathBuf {
    use object::write::Object;
    use object::{Architecture, BinaryFormat, Endianness};
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    for (section_name, data, kind) in sections {
        let section = object.add_section(vec![], section_name.as_bytes().to_vec(), *kind);
        object.append_section_data(section, data, 4);
    }
    let path =
        std::env::temp_dir().join(format!("mcu-run-image-{}-{name}.elf", std::process::id()));
    std::fs::write(&path, object.write().unwrap()).unwrap();
    path
}

#[test]
fn an_elf_names_its_chip_and_its_preferred_flash_algorithm() {
    let elf = elf_with(
        "both",
        &[
            (".probe-rs.chip", b"STM32H7S3L8"),
            (
                ".mcu-run.flash-algorithm",
                b"mx25uw25645g_stm32h7s38-nucleo",
            ),
        ],
    );
    let image = Image::from_elf(elf.clone()).unwrap();
    assert_eq!(
        image,
        Image {
            elf: elf.clone(),
            chip: "STM32H7S3L8".to_owned(),
            preferred_flash_algorithm: Some("mx25uw25645g_stm32h7s38-nucleo".to_owned()),
            ..Image::default()
        }
    );
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn an_elf_names_the_flash_mode_its_board_needs() {
    let elf = elf_with(
        "flash-mode",
        &[
            (".probe-rs.chip", b"esp32c3"),
            (".mcu-run.flash-mode", b"dio"),
        ],
    );
    assert_eq!(
        Image::from_elf(elf.clone()).unwrap().flash_mode,
        Some(FlashMode::Dio)
    );
    std::fs::remove_file(elf).unwrap();
    let elf = elf_with("no-flash-mode", &[(".probe-rs.chip", b"esp32c5")]);
    assert_eq!(Image::from_elf(elf.clone()).unwrap().flash_mode, None);
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn a_flash_mode_that_is_not_one_is_reported_with_its_section() {
    let elf = elf_with(
        "bad-flash-mode",
        &[
            (".probe-rs.chip", b"esp32c5"),
            (".mcu-run.flash-mode", b"fast"),
        ],
    );
    assert_eq!(
        Image::from_elf(elf.clone()).unwrap_err(),
        Error::Elf {
            path: elf.clone(),
            error:
                "the .mcu-run.flash-mode section: fast is not a flash mode: qio, qout, dio or dout"
                    .to_owned(),
        }
    );
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn an_elf_without_a_flash_algorithm_lets_probe_rs_choose() {
    let elf = elf_with("chip", &[(".probe-rs.chip", b"esp32s31")]);
    let image = Image::from_elf(elf.clone()).unwrap();
    assert_eq!(image.chip, "esp32s31");
    assert_eq!(image.preferred_flash_algorithm, None);
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn an_elf_that_names_no_chip_is_reported_with_its_path() {
    for (name, sections) in [
        ("none", &[][..]),
        ("empty", &[(".probe-rs.chip", &b""[..])][..]),
        (
            "algorithm-only",
            &[(".mcu-run.flash-algorithm", &b"stm32h7rx_64k"[..])][..],
        ),
    ] {
        let elf = elf_with(name, sections);
        let error = Image::from_elf(elf.clone()).unwrap_err();
        assert_eq!(error, Error::NoChip { elf: elf.clone() }, "{name}");
        assert_eq!(
            error.to_string(),
            format!(
                "{} names no chip in a .probe-rs.chip section",
                elf.display()
            )
        );
        std::fs::remove_file(elf).unwrap();
    }
}

#[test]
fn a_name_that_is_not_utf_8_is_reported_with_its_section() {
    let elf = elf_with("latin1", &[(".probe-rs.chip", b"esp32\xe9")]);
    let error = Image::from_elf(elf.clone()).unwrap_err();
    assert_eq!(
        error,
        Error::Elf {
            path: elf.clone(),
            error: "the .probe-rs.chip section is not UTF-8".to_owned(),
        }
    );
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn a_file_that_is_missing_or_not_an_elf_is_reported_with_its_path() {
    let missing = std::env::temp_dir().join("mcu-run-image-no-such-file.elf");
    assert!(
        matches!(Image::from_elf(missing.clone()), Err(Error::Elf { path, .. }) if path == missing),
    );
    let text = std::env::temp_dir().join(format!("mcu-run-image-{}-text.elf", std::process::id()));
    std::fs::write(&text, b"not an elf").unwrap();
    assert!(matches!(Image::from_elf(text.clone()), Err(Error::Elf { path, .. }) if path == text));
    std::fs::remove_file(text).unwrap();
}

#[test]
fn a_built_in_chip_resolves_with_the_name_the_description_spells() {
    let runner = Runner::new(image("ESP32C5", None)).unwrap();
    assert_eq!(runner.chip(), "esp32c5");
    assert_eq!(runner.image().chip, "ESP32C5");
    assert_eq!(
        Runner::new(image("stm32h7s3l8", None)).unwrap().chip(),
        "STM32H7S3L8"
    );
}

#[test]
fn an_unknown_chip_is_named() {
    let error = Runner::new(image("no-such-chip", None)).err().unwrap();
    assert!(
        matches!(&error, Error::UnknownChip { chip, .. } if chip == "no-such-chip"),
        "{error:?}"
    );
    assert!(
        error.to_string().starts_with("unknown chip no-such-chip:"),
        "{error}"
    );
}

#[test]
fn a_preferred_flash_algorithm_the_chip_does_not_have_is_named() {
    let mut wrong = image("STM32H7S3L8", None);
    wrong.preferred_flash_algorithm = Some("no-such-algorithm".to_owned());
    let error = Runner::new(wrong).err().unwrap();
    assert_eq!(
        error,
        Error::UnknownFlashAlgorithm {
            chip: "STM32H7S3L8".to_owned(),
            algorithm: "no-such-algorithm".to_owned(),
        }
    );
    let mut right = image("STM32H7S3L8", None);
    right.preferred_flash_algorithm = Some("mx25uw25645g_stm32h7s38-nucleo".to_owned());
    assert_eq!(Runner::new(right).unwrap().chip(), "STM32H7S3L8");
}

#[test]
fn a_loaded_description_replaces_the_built_in_family_of_the_same_name() {
    let loaded = Runner::new(image("mcu-run-test-chip", Some("family.yaml"))).unwrap();
    assert_eq!(loaded.chip(), "mcu-run-test-chip");
    let gone = Runner::new(image("esp32c5", Some("family.yaml")))
        .err()
        .unwrap();
    assert!(matches!(gone, Error::UnknownChip { .. }), "{gone:?}");
    assert!(Runner::new(image("STM32H7S3L8", Some("family.yaml"))).is_ok());
}

#[test]
fn a_description_that_is_missing_or_broken_is_reported_with_its_path() {
    let missing = Runner::new(image("esp32c5", Some("missing.yaml")))
        .err()
        .unwrap();
    assert!(
        matches!(&missing, Error::ChipDescription { path, .. } if path.ends_with("missing.yaml")),
        "{missing:?}"
    );
    let broken = Runner::new(image("esp32c5", Some("broken.yaml")))
        .err()
        .unwrap();
    assert!(
        matches!(&broken, Error::ChipDescription { path, .. } if path.ends_with("broken.yaml")),
        "{broken:?}"
    );
    let text = broken.to_string();
    assert!(text.contains("broken.yaml"), "{text}");
}

/// A note as lld writes it for `--build-id=sha1`: name size 4, description size 20, type 3, `GNU\0`, the ID.
fn build_id_note(id: &[u8; 20]) -> Vec<u8> {
    let mut note = Vec::new();
    note.extend_from_slice(&4u32.to_le_bytes());
    note.extend_from_slice(&20u32.to_le_bytes());
    note.extend_from_slice(&3u32.to_le_bytes());
    note.extend_from_slice(b"GNU\0");
    note.extend_from_slice(id);
    note
}

#[test]
fn an_image_s_id_is_the_elf_s_gnu_build_id_in_lower_case_hex() {
    let mut id = [0u8; 20];
    id[0] = 0x0a;
    id[19] = 0xf0;
    let note = build_id_note(&id);
    let elf = elf_with_kinds(
        "build-id",
        &[
            (
                ".probe-rs.chip",
                b"stm32h7s3l8hx",
                object::SectionKind::Metadata,
            ),
            (".note.gnu.build-id", &note, object::SectionKind::Note),
        ],
    );
    let image = Image::from_elf(elf.clone()).unwrap();
    assert_eq!(image.id(), Ok(Some(format!("0a{}f0", "00".repeat(18)))));
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn an_esp_idf_image_s_id_is_the_sha_256_of_its_elf_file_even_with_a_build_id() {
    let note = build_id_note(&[0x11; 20]);
    let elf = elf_with_kinds(
        "app-desc",
        &[
            (".probe-rs.chip", b"esp32s31", object::SectionKind::Metadata),
            (
                ".flash.appdesc",
                &[0; 256],
                object::SectionKind::ReadOnlyData,
            ),
            (".note.gnu.build-id", &note, object::SectionKind::Note),
        ],
    );
    let image = Image::from_elf(elf.clone()).unwrap();
    let hash: String = <sha2::Sha256 as sha2::Digest>::digest(std::fs::read(&elf).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hash.len(), 64);
    assert_eq!(image.id(), Ok(Some(hash)));
    std::fs::remove_file(elf).unwrap();
}

#[test]
fn an_elf_without_an_id_has_none_and_a_missing_file_fails() {
    let elf = elf_with("no-build-id", &[(".probe-rs.chip", b"esp32s31")]);
    let image = Image::from_elf(elf.clone()).unwrap();
    assert_eq!(image.id(), Ok(None));
    std::fs::remove_file(&elf).unwrap();
    assert!(
        matches!(image.id(), Err(Error::Elf { path, .. }) if path == elf),
        "a missing file is an error"
    );
}
