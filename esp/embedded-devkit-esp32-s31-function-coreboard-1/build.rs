//! Writes `mcu-run.x`, which names the chip in the ELF for `mcu-run` and probe-rs. `.cargo/config.toml` links
//! it with `-Tmcu-run.x`, and an application on this crate does the same.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out.join("mcu-run.x"), linker_script("esp32s31", None))?;
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

/// A linker script with the chip in `.probe-rs.chip` and the flash algorithm in `.mcu-run.flash-algorithm`. Each
/// section is `(INFO)`, so it stays in the ELF and is never written to the chip. The script also names the image
/// ID in the app descriptor `__mcu_image_id`, where the firmware reads the ID that its banner names (K-26, B-24).
fn linker_script(chip: &str, flash_algorithm: Option<&str>) -> String {
    let mut script = String::from("SECTIONS\n{\n");
    script.push_str(&section(".probe-rs.chip", chip));
    if let Some(algorithm) = flash_algorithm {
        script.push_str(&section(".mcu-run.flash-algorithm", algorithm));
    }
    script.push_str("}\n");
    script.push_str(IMAGE_ID);
    script
}

/// The image ID's symbol: `app_elf_sha256`, which espflash fills with the SHA-256 of the ELF file, 144 bytes into
/// the 256-byte app descriptor. The compiler cannot see a linker symbol's bytes, so it cannot fold a read of the
/// ID into the zeros that the ELF holds.
///
/// See <https://github.com/espressif/esp-idf/blob/v5.4/components/esp_app_format/include/esp_app_desc.h>.
const IMAGE_ID: &str = "__mcu_image_id = esp_app_desc + 144;\n\
    ASSERT(SIZEOF(.flash.appdesc) == 256, \"the app descriptor is not ESP-IDF's 256 bytes\");\n";

fn section(name: &str, text: &str) -> String {
    let bytes: String = text
        .bytes()
        .map(|byte| format!(" BYTE(0x{byte:02x})"))
        .collect();
    format!("  {name} (INFO) : {{{bytes} }}\n")
}
