//! `--name=value` flags and byte sizes for console commands (K-25).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use mcu_console::{flag, parse_size};

#[test]
fn a_flag_is_a_name_and_a_value_after_two_dashes_and_an_equals_sign() {
    assert_eq!(flag("--msgs=1000"), Some(("msgs", "1000")));
    assert_eq!(flag("--size="), Some(("size", "")));
    assert_eq!(flag("--subject=a=b"), Some(("subject", "a=b")));
}

#[test]
fn a_word_in_another_form_is_not_a_flag() {
    assert_eq!(flag("--msgs"), None);
    assert_eq!(flag("-msgs=1000"), None);
    assert_eq!(flag("msgs=1000"), None);
    assert_eq!(flag("--=1000"), None);
    assert_eq!(flag("1000"), None);
}

#[test]
fn a_size_is_bytes_with_no_suffix_a_b_or_a_kb_of_1024() {
    assert_eq!(parse_size("128"), Some(128));
    assert_eq!(parse_size("128B"), Some(128));
    assert_eq!(parse_size("4KB"), Some(4096));
    assert_eq!(parse_size("0"), Some(0));
}

#[test]
fn a_size_with_another_suffix_no_digits_or_too_many_bytes_is_refused() {
    for text in [
        "1k", "1kB", "1K", "1KiB", "1MB", "1 KB", "KB", "B", "", "-1", "+1", "1.5KB",
    ] {
        assert_eq!(parse_size(text), None, "{text}");
    }
    assert_eq!(parse_size("18446744073709551615"), Some(u64::MAX));
    assert_eq!(parse_size("18446744073709551616"), None);
    assert_eq!(parse_size("18014398509481984KB"), None);
}
