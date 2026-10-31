//! Hostile-input tests for the 0x05 decoders: every length, count and id read
//! from a file is checked before it sizes a slice, an allocation or a delta
//! sum, and a bad one is a clean error, never a panic or a wrong id.
//!
//! The wasm32 half of these fixes (`as usize` keeping the low 32 bits) cannot
//! run on a 64-bit test host; the same checked conversions cover it, and the
//! wasm32 clippy job compiles them.

use super::*;
use crate::dict::parse_meta;
use crate::triples::TripleBlock;
use std::sync::Mutex;

fn varint(v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, v);
    out
}

fn varints(vs: &[u64]) -> Vec<u8> {
    vs.iter().flat_map(|&v| varint(v)).collect()
}

/// Lengths that broke the old arithmetic: `pos + len` overflowed (a panic in
/// debug, a reversed slice in release), and the 2^32 ones wrapped on wasm32.
const HOSTILE_LENGTHS: [u64; 6] = [
    u64::MAX,
    u64::MAX - 1,
    1 << 63,
    isize::MAX as u64 + 1,
    1 << 32,
    (1 << 32) + 3,
];

#[test]
fn container_with_a_hostile_section_length_is_an_error() {
    for len in HOSTILE_LENGTHS {
        let mut c = varint(1);
        c.extend(varint(len));
        c.extend_from_slice(b"abc");
        let got = std::panic::catch_unwind(|| decode_container(&c, CODEC_NONE));
        assert!(
            matches!(got, Ok(Err(FileError::Container(_)))),
            "length {len:#x}: {got:?}"
        );
    }
}

#[test]
fn container_with_a_hostile_section_count_is_an_error() {
    for n in [u64::MAX, 1 << 40, 3] {
        let mut c = varint(n);
        c.extend(varint(1));
        c.push(7);
        assert!(decode_container(&c, CODEC_NONE).is_err(), "count {n:#x}");
    }
}

#[test]
fn a_valid_container_still_decodes() {
    let c = encode_container(&[b"one", b"", b"three"], CODEC_NONE);
    let got = decode_container(&c, CODEC_NONE).unwrap();
    assert_eq!(got, vec![b"one".to_vec(), Vec::new(), b"three".to_vec()]);
}

#[test]
fn checked_slice_refuses_overflowing_and_out_of_bounds_ranges() {
    let bytes = [1u8, 2, 3, 4];
    assert_eq!(checked_slice(&bytes, 1, 2), Some(&bytes[1..3]));
    assert_eq!(checked_slice(&bytes, 4, 0), Some(&[][..]));
    assert_eq!(checked_slice(&bytes, 3, 2), None);
    assert_eq!(checked_slice(&bytes, 5, 0), None);
    for len in HOSTILE_LENGTHS {
        assert_eq!(checked_slice(&bytes, 1, len), None, "len {len:#x}");
        assert_eq!(checked_slice(&bytes, len, 1), None, "offset {len:#x}");
    }
}

/// A tile directory of `(delta_min, span, len)` records.
fn tile_dir(entries: &[(u64, u64, u64)]) -> Vec<u8> {
    let mut out = varint(entries.len() as u64);
    for &(dmin, span, len) in entries {
        out.extend(varints(&[dmin, span, len]));
    }
    out
}

#[test]
fn tile_directory_refuses_ids_outside_u32() {
    let max = u64::from(u32::MAX);
    let bad = [
        // A delta past u32: `as u32` read it as 7.
        vec![(max + 1 + 7, 0, 0)],
        // A span past u32.
        vec![(0, 1 << 32, 0)],
        // Deltas whose sum leaves u32: `wrapping_add` came back to 0.
        vec![(max, 0, 0), (1, 0, 0)],
        // A span that pushes the maximum past u32.
        vec![(max - 1, 5, 0)],
    ];
    for entries in bad {
        let d = tile_dir(&entries);
        assert!(
            parse_tile_directory(&d, d.len() as u64).is_err(),
            "{entries:?}"
        );
    }
    // The edges of the id space still parse.
    let d = tile_dir(&[(max - 1, 1, 0)]);
    let e = parse_tile_directory(&d, d.len() as u64).unwrap();
    assert_eq!((e[0].min_a, e[0].max_a), (u32::MAX - 1, u32::MAX));
}

#[test]
fn tile_directory_refuses_a_hostile_tile_count_or_length() {
    let d = varints(&[u64::MAX, 0, 0, 0]);
    assert!(parse_tile_directory(&d, d.len() as u64).is_err());
    for len in HOSTILE_LENGTHS {
        let d = tile_dir(&[(0, 0, len)]);
        assert!(
            parse_tile_directory(&d, d.len() as u64).is_err(),
            "{len:#x}"
        );
    }
}

/// A reader over a section that claims to be huge: it records every read and
/// refuses any over 1 MiB instead of allocating it.
struct HugeSectionReader {
    reads: Mutex<Vec<(u64, u64)>>,
}

impl RangeReader for HugeSectionReader {
    fn len(&self) -> u64 {
        u64::MAX
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        if len > 1 << 20 {
            return Err(std::io::Error::other("refused a huge read"));
        }
        Ok(vec![0; len as usize])
    }
}

#[test]
fn tile_synopsis_trailer_read_is_bounded_by_the_tile_count() {
    let reader = HugeSectionReader {
        reads: Mutex::new(Vec::new()),
    };
    let dir = [
        TileDirEntry {
            min_a: 1,
            max_a: 1,
            start: 10,
            end: 20,
        },
        TileDirEntry {
            min_a: 2,
            max_a: 2,
            start: 20,
            end: 30,
        },
    ];
    let section = ByteRange {
        offset: 1000,
        len: 1 << 40,
    };
    let syn = read_tile_synopsis_ranged(&reader, section, &dir);
    // Four zero varints per tile: a parseable all-zero trailer.
    assert_eq!(syn, vec![Some((0, 0, 0, 0)); 2]);
    // Two tiles of four u32 varints are at most 40 bytes — not the 1 TiB the
    // section claimed after the last tile.
    assert_eq!(reader.reads.lock().unwrap().as_slice(), &[(1030, 40)]);
}

/// A chunk directory of `(delta_run, key, compressed_len)` records.
fn chunk_dir(chunks: &[(u64, &[u8], u64)]) -> Vec<u8> {
    let mut out = varint(chunks.len() as u64);
    for &(drun, key, clen) in chunks {
        out.extend(varints(&[drun, key.len() as u64]));
        out.extend_from_slice(key);
        out.extend(varint(clen));
    }
    out
}

fn parses(dir: &[u8], num_runs: usize) -> Result<bool, FileError> {
    match parse_chunk_dir_only(dir, 1 << 20, num_runs)? {
        ChunkDirParse::Done(_) => Ok(true),
        ChunkDirParse::Truncated { .. } => Ok(false),
    }
}

#[test]
fn dict_chunk_runs_must_ascend_from_zero_and_stay_in_range() {
    // Valid: chunks at runs 0, 2 and 5 of 6.
    let ok = chunk_dir(&[(0, b"", 1), (2, b"b", 1), (3, b"c", 1)]);
    assert!(matches!(parses(&ok, 6), Ok(true)));
    let bad = [
        // Chunk 0 must start at run 0.
        chunk_dir(&[(1, b"", 1)]),
        // A later chunk at the same run as the previous one: routing a term
        // id to its chunk is a binary search over ascending first runs.
        chunk_dir(&[(0, b"", 1), (0, b"b", 1)]),
        // A run past the section's runs.
        chunk_dir(&[(0, b"", 1), (6, b"b", 1)]),
        // A delta whose sum overflows.
        chunk_dir(&[(0, b"", 1), (3, b"b", 1), (u64::MAX - 1, b"c", 1)]),
    ];
    for dir in &bad {
        assert!(parses(dir, 6).is_err(), "{dir:?}");
    }
    // The full (local) parser applies the same rule.
    let header = varints(&[12, 2, 6, 0, 1, 2, 3, 4, 5]);
    for (dir, want_ok) in [(&ok, true), (&bad[1], false), (&bad[0], false)] {
        let mut payload = varint(header.len() as u64);
        payload.extend_from_slice(&header);
        payload.extend_from_slice(dir);
        let got = parse_chunked_dict_dir(&payload, payload.len() as u64 + 3);
        assert_eq!(got.is_ok(), want_ok, "{dir:?}");
    }
}

#[test]
fn dict_header_refuses_counts_outside_their_types() {
    // term_count, interval, num_restarts, then the restart offsets.
    assert!(parse_meta(&varints(&[4, 2, 2, 0, 3])).is_ok());
    // A term count past u32 (was truncated to 4).
    assert!(parse_meta(&varints(&[(1 << 32) + 4, 2, 2, 0, 3])).is_err());
    // An interval past u32 (was truncated to 2).
    assert!(parse_meta(&varints(&[4, (1 << 32) + 2, 2, 0, 3])).is_err());
    // A restart offset whose absolute position overflows.
    assert!(parse_meta(&varints(&[4, 2, 2, 0, u64::MAX])).is_err());
}

/// A triple block: the seven zone-map fields, then the grouped body.
fn block(zone: [u64; 7], body: &[u64]) -> Vec<u8> {
    let mut out = varints(&zone);
    out.extend(varints(body));
    out
}

#[test]
fn triple_block_refuses_ids_outside_u32() {
    // A zone-map field past u32.
    let bad = block([(1 << 32) + 1, 1, 0, 0, 0, 0, 1], &[1, 1, 1, 0, 1, 0]);
    assert!(TripleBlock::parse(&bad).is_err());

    // One a-group whose delta is 2^32 + 9: `as u32` decoded it as id 9.
    let wide = block([9, 9, 0, 0, 0, 0, 1], &[1, (1 << 32) + 9, 1, 0, 1, 0]);
    let b = TripleBlock::parse(&wide).unwrap();
    assert!(b.triples().is_empty());
    assert_eq!(b.scan(None, None, None).count(), 0);
    assert_eq!(b.scan(Some(9), None, None).count(), 0);

    // Two a-groups whose deltas leave the id space: `wrapping_add` turned
    // the second into id 0, out of order and wrong.
    let wrap = block(
        [u64::from(u32::MAX), 0, 0, 0, 0, 0, 2],
        &[2, u64::from(u32::MAX), 1, 0, 1, 0, 1, 1, 0, 1, 0],
    );
    let b = TripleBlock::parse(&wrap).unwrap();
    assert!(b.triples().is_empty());
    assert_eq!(
        b.scan(None, None, None).collect::<Vec<_>>(),
        vec![(u32::MAX, 0, 0)]
    );
    assert_eq!(b.group_directory().len(), 1);

    // The same shapes inside the id space decode.
    let fine = block([5, 6, 0, 0, 0, 0, 2], &[2, 5, 1, 0, 1, 0, 1, 1, 0, 1, 0]);
    let b = TripleBlock::parse(&fine).unwrap();
    assert_eq!(b.triples(), vec![(5, 0, 0), (6, 0, 0)]);
}
