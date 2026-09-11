#![forbid(unsafe_code)]

#[path = "../src/bulk_decode.rs"]
mod bulk_decode;

use bulk_decode::*;

fn record(common: u32, directory: u32, file: u32, name: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in [0, common, 0, directory, file, 0, 0, name.len() as u32] {
        bytes.extend(field.to_ne_bytes());
    }
    for (flag, value) in [(DEVID, 17u32), (OBJTYPE, 1), (ACCESSMASK, 0o755)] {
        if common & flag != 0 {
            bytes.extend(value.to_ne_bytes());
        }
    }
    if common & FILEID != 0 {
        bytes.extend(42u64.to_ne_bytes());
    }
    if directory & DIR_ALLOCSIZE != 0 {
        bytes.extend(4096i64.to_ne_bytes());
    }
    if file & LINKCOUNT != 0 {
        bytes.extend(2u32.to_ne_bytes());
    }
    if file & FILE_ALLOCSIZE != 0 {
        bytes.extend(8192i64.to_ne_bytes());
    }
    if file & DATALENGTH != 0 {
        bytes.extend(123i64.to_ne_bytes());
    }
    let offset = (bytes.len() - 24) as i32;
    bytes[24..28].copy_from_slice(&offset.to_ne_bytes());
    bytes.extend(name);
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    let length = bytes.len() as u32;
    bytes[..4].copy_from_slice(&length.to_ne_bytes());
    bytes
}

#[test]
fn decodes_optional_fields_and_non_utf8_names() {
    let flags = [
        DEVID,
        OBJTYPE,
        ACCESSMASK,
        FILEID,
        DIR_ALLOCSIZE,
        LINKCOUNT,
        FILE_ALLOCSIZE,
        DATALENGTH,
    ];
    for selection in 0..256 {
        let present = |bit| selection & (1 << bit) != 0;
        let common = (0..4)
            .filter(|&bit| present(bit))
            .fold(RETURNED_ATTRS | NAME, |mask, bit| mask | flags[bit]);
        let directory = if present(4) { DIR_ALLOCSIZE } else { 0 };
        let file = (5..8)
            .filter(|&bit| present(bit))
            .fold(0, |mask, bit| mask | flags[bit]);
        let bytes = record(common, directory, file, b"name-\xff\0");
        let (entry, length) = decode(&bytes).unwrap();
        assert_eq!(length, bytes.len());
        assert_eq!(entry.name.to_bytes(), b"name-\xff");
        assert_eq!(entry.devid, if present(0) { 17 } else { 0 });
        assert_eq!(entry.objtype, if present(1) { 1 } else { 0 });
        assert_eq!(entry.accessmask, if present(2) { 0o755 } else { 0 });
        assert_eq!(entry.fileid, if present(3) { 42 } else { 0 });
        assert_eq!(entry.linkcount, if present(5) { 2 } else { 0 });
        assert_eq!(entry.bytes, if present(7) { 123 } else { 0 });
        assert_eq!(
            entry.allocated,
            if present(6) {
                8192
            } else if present(4) {
                4096
            } else {
                entry.bytes
            }
        );
    }
}

#[test]
fn rejects_truncated_fields_and_records() {
    let bytes = record(COMMON, DIR_ALLOCSIZE, FILE, b"name\0");
    for length in 0..bytes.len() {
        assert!(decode(&bytes[..length]).is_none(), "truncated at {length}");
        if length >= 4 {
            let mut short = bytes[..length].to_vec();
            short[..4].copy_from_slice(&(length as u32).to_ne_bytes());
            // Only the final alignment padding may be absent.
            if length < bytes.len() - 3 {
                assert!(decode(&short).is_none(), "declared {length}");
            }
        }
    }
}

#[test]
fn bounds_names_to_their_record() {
    let good = record(COMMON, 0, FILE, b"name\0");
    for offset in [i32::MIN, -25, -24, -1, 0, 4, i32::MAX] {
        let mut bytes = good.clone();
        bytes[24..28].copy_from_slice(&offset.to_ne_bytes());
        assert!(decode(&bytes).is_none(), "offset {offset}");
    }
    for length in [0, 1, u32::MAX] {
        let mut bytes = good.clone();
        bytes[28..32].copy_from_slice(&length.to_ne_bytes());
        assert!(decode(&bytes).is_none(), "name length {length}");
    }
    for name in [b"\0".as_slice(), b"na\0me\0", b"name!"] {
        assert!(decode(&record(COMMON, 0, FILE, name)).is_none());
    }
    let mut batch = good.clone();
    batch.extend(&good);
    batch[24..28].copy_from_slice(&(good.len() as i32 - 24).to_ne_bytes());
    assert!(decode(&batch).is_none(), "name escapes into next record");
}

#[test]
fn rejects_missing_required_or_unrequested_attributes_and_negative_sizes() {
    for (position, value) in [
        (0, 0u32),
        (0, u32::MAX),
        (4, NAME),
        (4, COMMON | 4),
        (8, 1),
        (12, 1),
        (16, FILE | 2),
        (20, 1),
    ] {
        let mut bytes = record(COMMON, 0, FILE, b"name\0");
        bytes[position..position + 4].copy_from_slice(&value.to_ne_bytes());
        assert!(decode(&bytes).is_none(), "field {position}, value {value}");
    }
    let mut bytes = record(RETURNED_ATTRS | NAME, 0, DATALENGTH, b"name\0");
    bytes[32..40].copy_from_slice(&(-1i64).to_ne_bytes());
    assert!(decode(&bytes).is_none());
}

#[test]
fn advances_over_multiple_records() {
    let first = record(COMMON, 0, FILE, b"one\0");
    let second = record(COMMON, DIR_ALLOCSIZE, 0, b"two\0");
    let bytes = [first, second].concat();
    let (one, length) = decode(&bytes).unwrap();
    let (two, rest) = decode(&bytes[length..]).unwrap();
    assert_eq!(one.name.to_bytes(), b"one");
    assert_eq!(two.name.to_bytes(), b"two");
    assert_eq!(length + rest, bytes.len());
}

fn fuzz(cases: usize, mut state: u64) {
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 32
    };
    let seeds = [
        record(COMMON, 0, FILE, b"name-\xff\0"),
        record(COMMON, DIR_ALLOCSIZE, 0, b"directory\0"),
        record(NAME | RETURNED_ATTRS, 0, DATALENGTH, b"short\0"),
    ];
    for iteration in 0..cases {
        let mut bytes = if iteration % 4 == 0 {
            (0..next() as usize % 256)
                .map(|_| next() as u8)
                .collect::<Vec<_>>()
        } else {
            seeds[iteration % seeds.len()].clone()
        };
        for _ in 0..next() % 8 {
            if bytes.is_empty() {
                break;
            }
            let index = next() as usize % bytes.len();
            bytes[index] = next() as u8;
        }
        if iteration % 7 == 0 {
            bytes.truncate(next() as usize % (bytes.len() + 1));
        }
        if let Some((entry, length)) = decode(&bytes) {
            assert!(length >= 32 && length <= bytes.len());
            let offset = (entry.name.as_ptr() as usize)
                .checked_sub(bytes.as_ptr() as usize)
                .unwrap();
            assert!(offset + entry.name.to_bytes_with_nul().len() <= length);
            assert!(!entry.name.is_empty());
        }
    }
}

#[test]
fn seeded_decoder_mutations() {
    fuzz(20_000, 0x5afe_2026);
}

#[test]
#[ignore = "cargo test -p size --test bulk_decode fuzz_decoder -- --ignored"]
fn fuzz_decoder() {
    let cases = std::env::var("SIZE_FUZZ_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let seed = std::env::var("SIZE_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x5afe_2026);
    fuzz(cases, seed);
}

#[cfg(target_vendor = "apple")]
#[test]
fn darwin_constants_match_the_native_abi() {
    assert_eq!(
        [
            NAME,
            DEVID,
            OBJTYPE,
            ACCESSMASK,
            FILEID,
            RETURNED_ATTRS,
            DIR_ALLOCSIZE,
            LINKCOUNT,
            FILE_ALLOCSIZE,
            DATALENGTH
        ],
        [
            libc::ATTR_CMN_NAME,
            libc::ATTR_CMN_DEVID,
            libc::ATTR_CMN_OBJTYPE,
            libc::ATTR_CMN_ACCESSMASK,
            libc::ATTR_CMN_FILEID,
            libc::ATTR_CMN_RETURNED_ATTRS,
            libc::ATTR_DIR_ALLOCSIZE,
            libc::ATTR_FILE_LINKCOUNT,
            libc::ATTR_FILE_ALLOCSIZE,
            libc::ATTR_FILE_DATALENGTH
        ]
    );
    assert_eq!(std::mem::size_of::<libc::attribute_set_t>(), 20);
    assert_eq!(std::mem::size_of::<libc::attrreference_t>(), 8);
}
