//! Writes the board's `memory.x`, which cortex-m-rt's `link.x` includes (B-6), and `mcu-run.x`, which names the
//! chip and its flash algorithm in the ELF for `mcu-run` and probe-rs and places the image's build ID.
//! `.cargo/config.toml` links `mcu-run.x` with `-Tmcu-run.x` and `--build-id=sha1`, and an application on this
//! crate does the same.
//!
//! The chip boots from internal flash, which holds the vector table, cortex-m-rt's `Reset` and the start-up code in
//! `.boot`. Every other function and all read-only data live in the external flash at `0x70000000`, which the
//! start-up code maps into memory before `Reset` reaches `main`. The sizes come from reads of the board: a read at
//! the end of DTCM's 64 KB succeeds, and one past it fails. embassy-stm32's `memory-x` feature is not used, because
//! its stm32-data source gives DTCM as 192 KB.

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

const MEMORY: &str = r#"
MEMORY
{
  FLASH  : ORIGIN = 0x08000000, LENGTH = 64K
  XFLASH : ORIGIN = 0x70000000, LENGTH = 32M
  RAM    : ORIGIN = 0x20000000, LENGTH = 64K
  AXISRAM : ORIGIN = 0x24000000, LENGTH = 456K
}

/* An output section that comes earlier takes the input sections it matches first, so these take every function and
   all read-only data before cortex-m-rt's .text and .rodata can. .text keeps .Reset and the fault handlers. */
SECTIONS
{
  .xtext : ALIGN(4)
  {
    *(.text .text.*);
    . = ALIGN(4);
  } > XFLASH

  .xrodata : ALIGN(4)
  {
    *(.rodata .rodata.*);
    . = ALIGN(4);
  } > XFLASH
} INSERT BEFORE .text;

SECTIONS
{
  .boot : ALIGN(4)
  {
    KEEP(*(.boot .boot.*));
    . = ALIGN(4);
  } > FLASH
} INSERT AFTER .text;

/* The start-up code cannot read external flash, and only .xrodata is there. A linker that matched sections in
   another order would leave read-only data in internal flash. */
ASSERT(SIZEOF(.rodata) == 0, "read-only data landed in internal flash");
"#;

/// The chip in probe-rs.
const CHIP: &str = "STM32H7S3L8";

/// probe-rs also gives the chip the STM32H7S78-DK's algorithm for the same external flash, which times out in its
/// initialisation on this board.
const FLASH_ALGORITHM: &str = "mx25uw25645g_stm32h7s38-nucleo";

fn main() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out.join("memory.x"), MEMORY)?;
    fs::write(
        out.join("mcu-run.x"),
        linker_script(CHIP, Some(FLASH_ALGORITHM), "FLASH"),
    )?;
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

/// A linker script with the chip in `.probe-rs.chip` and the flash algorithm in `.mcu-run.flash-algorithm`. Each
/// section is `(INFO)`, so it stays in the ELF and is never written to the chip. The script also puts the GNU
/// build ID in `region` at `__mcu_build_id`, where the firmware reads the ID that its banner names (K-26, B-24).
/// The region is internal flash, which the start-up code does not have to map.
fn linker_script(chip: &str, flash_algorithm: Option<&str>, region: &str) -> String {
    let mut script = String::from("SECTIONS\n{\n");
    script.push_str(&section(".probe-rs.chip", chip));
    if let Some(algorithm) = flash_algorithm {
        script.push_str(&section(".mcu-run.flash-algorithm", algorithm));
    }
    script.push_str("}\n");
    script.push_str(&build_id(region));
    script
}

/// The build ID's placement. A section that the main script does not name can land on top of another, so the
/// note is placed after `.rodata`. The link fails without `--build-id=sha1`, because the firmware reads 36 bytes
/// there.
fn build_id(region: &str) -> String {
    format!(
        "SECTIONS\n{{\n  .note.gnu.build-id : ALIGN(4)\n  {{\n    __mcu_build_id = .;\n    \
         KEEP(*(.note.gnu.build-id));\n  }} > {region}\n}} INSERT AFTER .rodata;\n\
         ASSERT(SIZEOF(.note.gnu.build-id) == 36, \"link with -C link-arg=--build-id=sha1\");\n"
    )
}

fn section(name: &str, text: &str) -> String {
    let bytes: String = text
        .bytes()
        .map(|byte| format!(" BYTE(0x{byte:02x})"))
        .collect();
    format!("  {name} (INFO) : {{{bytes} }}\n")
}
