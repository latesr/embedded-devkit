//! Reading the GNU build ID out of the note the linker writes, as the application finds it in flash.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use mcu_console::gnu_build_id;

const ID: [u8; 20] = [
    0x12, 0x33, 0x77, 0x06, 0xfd, 0xa3, 0x1f, 0x42, 0x2a, 0x70, 0xcb, 0x98, 0x93, 0xf0, 0x1b, 0x79,
    0x44, 0xae, 0x10, 0x7f,
];

/// A note as lld writes it for `--build-id=sha1`: name size 4, description size 20, type 3, `GNU\0`, the ID.
fn note() -> Vec<u8> {
    let mut note = Vec::new();
    note.extend_from_slice(&4u32.to_le_bytes());
    note.extend_from_slice(&20u32.to_le_bytes());
    note.extend_from_slice(&3u32.to_le_bytes());
    note.extend_from_slice(b"GNU\0");
    note.extend_from_slice(&ID);
    note
}

#[test]
fn the_id_is_the_note_s_description() {
    assert_eq!(gnu_build_id(&note()), Some(&ID[..]));
}

#[test]
fn bytes_after_the_note_are_not_part_of_the_id() {
    let mut padded = note();
    padded.extend_from_slice(&[0xff; 8]);
    assert_eq!(gnu_build_id(&padded), Some(&ID[..]));
}

#[test]
fn a_note_of_another_owner_or_type_has_no_build_id() {
    let mut owner = note();
    owner[12..16].copy_from_slice(b"GNV\0");
    assert_eq!(gnu_build_id(&owner), None);
    let mut kind = note();
    kind[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(gnu_build_id(&kind), None);
    let mut name_size = note();
    name_size[0..4].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(gnu_build_id(&name_size), None);
}

#[test]
fn a_note_that_ends_early_or_claims_more_than_it_holds_has_no_build_id() {
    let whole = note();
    for len in [0, 11, 15, whole.len() - 1] {
        assert_eq!(gnu_build_id(&whole[..len]), None, "{len} bytes");
    }
    let mut long = note();
    long[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(gnu_build_id(&long), None);
}

#[test]
fn an_empty_description_has_no_build_id() {
    let mut empty = note();
    empty[4..8].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(gnu_build_id(&empty), None);
}
