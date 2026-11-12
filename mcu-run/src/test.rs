//! The embedded-test protocol: the tests in an ELF, and the outcome of running one.
//!
//! embedded-test 0.7 stores each test as a 12-byte global data symbol in the `.embedded_test` section: the
//! test function's address, then the address and length of the module path string. The symbol's name is a JSON
//! object with the test's name, `should_panic`, `ignored` and `timeout`. The `EMBEDDED_TEST_VERSION` symbol in
//! the same section holds the protocol version, and this module reads version 1. Read from
//! `probe-rs-tools/src/bin/probe-rs/cmd/run.rs` at `v0.32.0` and embedded-test 0.7.2's `src/export.rs`.

use crate::Error;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use serde::Deserialize;

/// The outcome of one embedded-test test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestResult {
    /// The test ended as its attributes said it should.
    Passed {
        /// The test's name, with its module path.
        name: String,
    },
    /// The test ended another way, timed out or halted the core.
    Failed {
        /// The test's name, with its module path.
        name: String,
        /// Why it failed.
        reason: String,
    },
    /// The test has `#[ignore]` and did not run.
    Ignored {
        /// The test's name, with its module path.
        name: String,
    },
}

impl TestResult {
    /// The test's name.
    pub fn name(&self) -> &str {
        match self {
            TestResult::Passed { name }
            | TestResult::Failed { name, .. }
            | TestResult::Ignored { name } => name,
        }
    }
}

/// One test read from the ELF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TestCase {
    pub(crate) name: String,
    /// The test function's address, which the run command line names.
    pub(crate) address: u32,
    pub(crate) should_panic: bool,
    pub(crate) ignored: bool,
    /// The test's own timeout in seconds, when it has one.
    pub(crate) timeout: Option<u32>,
}

/// The JSON in a test symbol's name.
#[derive(Deserialize)]
struct Definition {
    name: String,
    should_panic: bool,
    ignored: bool,
    timeout: Option<u32>,
}

/// The size of a test symbol: a function pointer and a string slice, on a 32-bit target.
const TEST_SYMBOL_SIZE: u64 = 12;

/// The tests in `elf`, or `None` when the ELF is not an embedded-test binary. Fails on a protocol version other
/// than 1 and on a test symbol that cannot be read.
pub(crate) fn tests_in_elf(elf: &[u8]) -> Result<Option<Vec<TestCase>>, Error> {
    let file = object::File::parse(elf)
        .map_err(|error| Error::TestList(format!("the image is not an ELF: {error}")))?;
    let Some(section) = file.section_by_name(".embedded_test") else {
        return Ok(None);
    };
    let Some(version) = file.symbol_by_name("EMBEDDED_TEST_VERSION") else {
        return Ok(None);
    };
    let version = section
        .data_range(version.address(), version.size())
        .ok()
        .flatten()
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| {
            Error::TestList("the EMBEDDED_TEST_VERSION symbol cannot be read".to_owned())
        })?;
    if version != 1 {
        return Err(Error::TestList(format!(
            "embedded-test protocol version {version} is not supported, only version 1 is"
        )));
    }
    let mut tests = Vec::new();
    for symbol in file.symbols() {
        if !symbol.is_global()
            || symbol.kind() != SymbolKind::Data
            || symbol.section_index() != Some(section.index())
            || symbol.size() != TEST_SYMBOL_SIZE
        {
            continue;
        }
        let data = section
            .data_range(symbol.address(), TEST_SYMBOL_SIZE)
            .ok()
            .flatten()
            .ok_or_else(|| {
                Error::TestList(format!(
                    "test symbol at {:#x} cannot be read",
                    symbol.address()
                ))
            })?;
        let word = |at: usize| -> u32 {
            data.get(at..at.saturating_add(4))
                .and_then(|bytes| bytes.try_into().ok())
                .map(u32::from_le_bytes)
                .unwrap_or(0)
        };
        let address = word(0);
        let module = read_string(&file, elf, word(4), word(8))?;
        let name = symbol
            .name()
            .map_err(|error| Error::TestList(error.to_string()))?;
        tests.push(test_case(name, &module, address)?);
    }
    tests.sort_by_key(|test| test.address);
    Ok(Some(tests))
}

/// A test from its symbol's JSON name, the module path string the symbol points at, and its address.
fn test_case(json: &str, module: &str, address: u32) -> Result<TestCase, Error> {
    let definition: Definition = serde_json::from_str(json)
        .map_err(|error| Error::TestList(format!("test symbol {json}: {error}")))?;
    // The module path starts with the crate's name, which every test shares.
    let module = module.split_once("::").map_or(module, |(_, rest)| rest);
    Ok(TestCase {
        name: format!("{module}::{}", definition.name),
        address,
        should_panic: definition.should_panic,
        ignored: definition.ignored,
        timeout: definition.timeout,
    })
}

/// The string of `len` bytes at `address` in the ELF's file image.
fn read_string(
    file: &object::File<'_>,
    elf: &[u8],
    address: u32,
    len: u32,
) -> Result<String, Error> {
    let (address, len) = (u64::from(address), u64::from(len));
    let section = file
        .sections()
        .find(|section| {
            address >= section.address()
                && address.saturating_add(len) <= section.address().saturating_add(section.size())
        })
        .ok_or_else(|| Error::TestList(format!("no section holds the string at {address:#x}")))?;
    let (start, _) = section
        .file_range()
        .ok_or_else(|| Error::TestList(format!("the section at {address:#x} has no file data")))?;
    let from = usize::try_from(start.saturating_add(address.saturating_sub(section.address())))
        .map_err(|_| Error::TestList("string offset out of range".to_owned()))?;
    let to = from.saturating_add(usize::try_from(len).unwrap_or(usize::MAX));
    let bytes = elf.get(from..to).ok_or_else(|| {
        Error::TestList(format!("the string at {address:#x} is outside the file"))
    })?;
    String::from_utf8(bytes.to_vec()).map_err(|error| Error::TestList(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_symbol_gives_the_test_with_its_module_path_and_without_the_crate_name() {
        let test = test_case(
            r#"{"name":"one_and_one_are_two","should_panic":false,"ignored":false,"timeout":null}"#,
            "smoke::tests",
            0x4200_1234,
        )
        .unwrap();
        assert_eq!(
            test,
            TestCase {
                name: "tests::one_and_one_are_two".to_owned(),
                address: 0x4200_1234,
                should_panic: false,
                ignored: false,
                timeout: None,
            }
        );
    }

    #[test]
    fn the_attributes_are_read_and_a_module_path_without_a_crate_is_kept_whole() {
        let test = test_case(
            r#"{"name":"slow","should_panic":true,"ignored":true,"timeout":5}"#,
            "tests",
            8,
        )
        .unwrap();
        assert_eq!(test.name, "tests::slow");
        assert!(test.should_panic);
        assert!(test.ignored);
        assert_eq!(test.timeout, Some(5));
    }

    #[test]
    fn a_symbol_name_that_is_not_the_json_is_reported() {
        let error = test_case("not json", "tests", 0).err().unwrap();
        assert!(
            matches!(&error, Error::TestList(text) if text.starts_with("test symbol not json:")),
            "{error:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_an_elf_is_reported() {
        let error = tests_in_elf(b"not an elf at all").err().unwrap();
        assert!(
            matches!(&error, Error::TestList(text) if text.starts_with("the image is not an ELF:")),
            "{error:?}"
        );
    }

    #[test]
    fn each_result_gives_its_name() {
        let name = || "tests::a".to_owned();
        assert_eq!(TestResult::Passed { name: name() }.name(), "tests::a");
        assert_eq!(
            TestResult::Failed {
                name: name(),
                reason: String::new()
            }
            .name(),
            "tests::a"
        );
        assert_eq!(TestResult::Ignored { name: name() }.name(), "tests::a");
    }

    use object::write::{Object, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolScope};

    /// A test symbol for the fixture: its JSON name, its address, and whether it is global and 12 bytes.
    struct Sym {
        json: &'static str,
        address: u32,
        global: bool,
        size: u64,
        /// The symbol sits in `.rodata` instead of `.embedded_test`.
        elsewhere: bool,
    }

    /// An ELF with an `.embedded_test` section that holds the version, the module path string and one symbol
    /// per entry of `symbols`, the way embedded-test lays them out.
    fn fixture(version: Option<u32>, module: &str, symbols: &[Sym]) -> Vec<u8> {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let section = object.add_section(vec![], b".embedded_test".to_vec(), SectionKind::Data);
        let rodata = object.add_section(vec![], b".rodata".to_vec(), SectionKind::ReadOnlyData);
        let symbol = |name: &str, value: u64, size: u64, global: bool, elsewhere: bool| Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size,
            kind: SymbolKind::Data,
            scope: if global {
                SymbolScope::Dynamic
            } else {
                SymbolScope::Compilation
            },
            weak: false,
            section: SymbolSection::Section(if elsewhere { rodata } else { section }),
            flags: SymbolFlags::None,
        };
        if let Some(version) = version {
            let at = object.append_section_data(section, &version.to_le_bytes(), 4);
            object.add_symbol(symbol("EMBEDDED_TEST_VERSION", at, 4, true, false));
        }
        let module_at = object.append_section_data(section, module.as_bytes(), 1);
        for sym in symbols {
            let mut data = sym.address.to_le_bytes().to_vec();
            data.extend((module_at as u32).to_le_bytes());
            data.extend((module.len() as u32).to_le_bytes());
            data.extend([0; 4]);
            let target = if sym.elsewhere { rodata } else { section };
            let at = object.append_section_data(target, &data, 4);
            object.add_symbol(symbol(sym.json, at, sym.size, sym.global, sym.elsewhere));
        }
        object.write().unwrap()
    }

    const PLAIN: &str = r#"{"name":"plain","should_panic":false,"ignored":false,"timeout":null}"#;
    const PANICS: &str = r#"{"name":"panics","should_panic":true,"ignored":false,"timeout":7}"#;

    fn sym(json: &'static str, address: u32) -> Sym {
        Sym {
            json,
            address,
            global: true,
            size: TEST_SYMBOL_SIZE,
            elsewhere: false,
        }
    }

    #[test]
    fn the_tests_are_read_from_the_elf_in_address_order() {
        let elf = fixture(
            Some(1),
            "smoke::tests",
            &[sym(PANICS, 0x4200_0200), sym(PLAIN, 0x4200_0100)],
        );
        let tests = tests_in_elf(&elf).unwrap().unwrap();
        assert_eq!(
            tests,
            [
                TestCase {
                    name: "tests::plain".to_owned(),
                    address: 0x4200_0100,
                    should_panic: false,
                    ignored: false,
                    timeout: None,
                },
                TestCase {
                    name: "tests::panics".to_owned(),
                    address: 0x4200_0200,
                    should_panic: true,
                    ignored: false,
                    timeout: Some(7),
                },
            ]
        );
    }

    #[test]
    fn a_local_symbol_or_one_of_another_size_or_section_is_not_a_test() {
        let elf = fixture(
            Some(1),
            "smoke::tests",
            &[
                Sym {
                    json: PLAIN,
                    address: 1,
                    global: false,
                    size: TEST_SYMBOL_SIZE,
                    elsewhere: false,
                },
                Sym {
                    json: PANICS,
                    address: 2,
                    global: true,
                    size: 16,
                    elsewhere: false,
                },
                Sym {
                    json: PLAIN,
                    address: 3,
                    global: true,
                    size: TEST_SYMBOL_SIZE,
                    elsewhere: true,
                },
            ],
        );
        assert_eq!(tests_in_elf(&elf).unwrap().unwrap(), []);
    }

    #[test]
    fn a_section_without_the_version_symbol_is_not_a_test_binary() {
        let elf = fixture(None, "smoke::tests", &[sym(PLAIN, 1)]);
        assert_eq!(tests_in_elf(&elf).unwrap(), None);
    }

    #[test]
    fn another_protocol_version_is_refused() {
        let elf = fixture(Some(2), "smoke::tests", &[]);
        let error = tests_in_elf(&elf).err().unwrap();
        assert!(
            matches!(&error, Error::TestList(text) if text.contains("version 2")),
            "{error:?}"
        );
    }

    #[test]
    fn a_module_path_that_points_outside_every_section_is_reported() {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let section = object.add_section(vec![], b".embedded_test".to_vec(), SectionKind::Data);
        let at = object.append_section_data(section, &1u32.to_le_bytes(), 4);
        object.add_symbol(Symbol {
            name: b"EMBEDDED_TEST_VERSION".to_vec(),
            value: at,
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Dynamic,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        let mut data = 1u32.to_le_bytes().to_vec();
        data.extend(0x1000_0000u32.to_le_bytes());
        data.extend(4u32.to_le_bytes());
        let at = object.append_section_data(section, &data, 4);
        object.add_symbol(Symbol {
            name: PLAIN.as_bytes().to_vec(),
            value: at,
            size: TEST_SYMBOL_SIZE,
            kind: SymbolKind::Data,
            scope: SymbolScope::Dynamic,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        let error = tests_in_elf(&object.write().unwrap()).err().unwrap();
        assert!(
            matches!(&error, Error::TestList(text) if text.starts_with("no section holds the string")),
            "{error:?}"
        );
    }
}
