//! The byte-level primitives of the HDT container format.
//!
//! Everything here is dictated by what `hdt-cpp` reads, not by the W3C member
//! submission, which is vaguer than the implementation in every place that
//! matters. The citations are to the `hdt-cpp` tree the reference binaries were
//! built from (`libhdt/`, commit b90d8a3); each was re-checked against upstream
//! HEAD, which differs only in formatting.
//!
//! Three of these details are easy to get wrong in a way that produces a file
//! `hdt-cpp` rejects with no useful diagnostic, so they are called out where they
//! are implemented:
//!
//! * **VByte sets the high bit on the LAST byte**, not on continuation bytes —
//!   the opposite of the convention `rete`'s own `varint` module uses. Do not
//!   reuse that one here.
//! * **The data CRC is CRC-32C (Castagnoli)**, not the zlib CRC-32.
//! * Each CRC covers an exactly delimited byte range, and the header CRC of a
//!   structure never covers that structure's payload.

// ---------------------------------------------------------------------------
// VByte  —  libhdt/src/libdcs/VByte.cpp:38-53
// ---------------------------------------------------------------------------

/// Append `value` in HDT's VByte encoding.
///
/// Seven payload bits per byte, least-significant group first, and **the high
/// bit marks the final byte**:
///
/// ```text
///     while value > 127 { emit(value & 127); value >>= 7 }
///     emit(value | 0x80)
/// ```
///
/// So `0` is the single byte `0x80`, `127` is `0xFF`, and `128` is `00 81`. The
/// continuation-bit convention — high bit set on every byte *except* the last —
/// is what `rete_core::varint` uses and what most readers expect; writing that
/// here produces a dictionary `hdt-cpp` silently misreads.
pub(crate) fn vbyte(out: &mut Vec<u8>, mut value: u64) {
    while value > 127 {
        out.push((value & 127) as u8);
        value >>= 7;
    }
    out.push((value | 0x80) as u8);
}

// ---------------------------------------------------------------------------
// CRCs  —  libhdt/src/util/crc8.h, crc16.h, crc32.h
// ---------------------------------------------------------------------------

/// CRC-8/SMBUS: poly 0x07, init 0x00, not reflected, no final XOR.
/// `libhdt/src/util/crc8.h:8-14`.
pub(crate) fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16/ARC: poly 0x8005 reflected (0xA001), init 0x0000, reflected in and
/// out, no final XOR. `libhdt/src/util/crc16.h:8-14`. Written little-endian.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// CRC-32**C** (Castagnoli): poly 0x1EDC6F41 reflected (0x82F63B78), init and
/// final XOR 0xFFFFFFFF, reflected. `libhdt/src/util/crc32.h:8-14`.
///
/// **Not** the zlib/PKZIP CRC-32 (poly 0x04C11DB7). Every data block in an HDT
/// is checked with this one, so using the familiar polynomial fails the file on
/// the first section.
pub(crate) fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// ControlInformation  —  libhdt/src/hdt/ControlInformation.cpp:46-75
// ---------------------------------------------------------------------------

/// `ControlInformationType`, `libhdt/include/ControlInformation.hpp:46-53`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CiType {
    Global = 1,
    Header = 2,
    Dictionary = 3,
    Triples = 4,
}

/// Serialize one control-information block.
///
/// ```text
///     "$HDT" | type | format | NUL | properties | NUL | crc16(LE)
/// ```
///
/// `properties` is the concatenation of `key=value;` — **every** pair ends with
/// the semicolon, including the last, and an empty set is just the terminating
/// NUL. `hdt-cpp` stores them in a `std::map`, so it writes them in ascending
/// key order; we take them pre-sorted for the same reason, since a reader that
/// round-trips the file should get the same bytes back.
///
/// The CRC16 covers everything from the `$` of the cookie through the final NUL,
/// and not itself.
pub(crate) fn control_info(
    out: &mut Vec<u8>,
    kind: CiType,
    format: &str,
    properties: &[(&str, String)],
) {
    let start = out.len();
    out.extend_from_slice(b"$HDT");
    out.push(kind as u8);
    out.extend_from_slice(format.as_bytes());
    out.push(0);
    for (k, v) in properties {
        out.extend_from_slice(k.as_bytes());
        out.push(b'=');
        out.extend_from_slice(v.as_bytes());
        out.push(b';');
    }
    out.push(0);
    let crc = crc16(&out[start..]);
    out.extend_from_slice(&crc.to_le_bytes());
}

/// The format strings, `libhdt/include/HDTVocabulary.hpp`.
pub(crate) const GLOBAL_FORMAT: &str = "<http://purl.org/HDT/hdt#HDTv1>";
pub(crate) const HEADER_FORMAT: &str = "ntriples";
pub(crate) const DICTIONARY_FORMAT: &str = "<http://purl.org/HDT/hdt#dictionaryFour>";
pub(crate) const TRIPLES_FORMAT: &str = "<http://purl.org/HDT/hdt#triplesBitmap>";

/// `TripleComponentOrder::SPO`, `libhdt/include/HDTEnums.hpp:76-96`.
pub(crate) const ORDER_SPO: u32 = 1;

// ---------------------------------------------------------------------------
// LogSequence2  —  libhdt/src/sequence/LogSequence2.cpp:294-321
// ---------------------------------------------------------------------------

/// Bits needed to represent `n`; `bits(0) == 0`.
/// `libhdt/src/sequence/LogSequence2.hpp:217-221`.
pub(crate) fn bits_for(n: u64) -> u8 {
    (64 - n.leading_zeros()) as u8
}

/// A fixed-width packed integer array, HDT's `LogSequence2`.
///
/// ```text
///     0x01 | numbits(1 raw byte) | vbyte(numentries) | crc8 | data | crc32c(LE)
/// ```
///
/// `numbits` is a bit *width* and is written as one raw byte — note the contrast
/// with [`BitSequence`] (HDT's `BitSequence375`), whose length field is a VByte. The data is a flat
/// LSB-first bitstream: entry `i` occupies bits `[i*numbits, (i+1)*numbits)`,
/// low-order bit at the lowest position. `hdt-cpp` packs into `size_t` words,
/// but the machine is little-endian so the word structure is invisible on disk.
///
/// `numbytes` is `ceil(numbits * numentries / 8)` — not word-padded. Bits past
/// the last entry are never read (`hdt-cpp` leaves stale data there after
/// `reduceBits`); we write zeros.
pub(crate) struct LogSequence {
    numbits: u8,
    numentries: u64,
    data: Vec<u8>,
}

impl LogSequence {
    /// Pack `values`, sizing the field width to the largest of them.
    #[cfg(test)]
    pub(crate) fn new(values: &[u64]) -> Self {
        let max = values.iter().copied().max().unwrap_or(0);
        Self::with_bits(values, bits_for(max))
    }

    pub(crate) fn with_bits(values: &[u64], numbits: u8) -> Self {
        let numentries = values.len() as u64;
        let total_bits = numbits as u64 * numentries;
        let numbytes = total_bits.div_ceil(8) as usize;
        let mut data = vec![0u8; numbytes];
        if numbits > 0 {
            for (i, &v) in values.iter().enumerate() {
                let start = i as u64 * numbits as u64;
                for b in 0..numbits as u64 {
                    if (v >> b) & 1 == 1 {
                        let pos = start + b;
                        data[(pos / 8) as usize] |= 1 << (pos % 8);
                    }
                }
            }
        }
        Self {
            numbits,
            numentries,
            data,
        }
    }

    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        let mut head = Vec::with_capacity(11);
        head.push(0x01u8); // TYPE_SEQLOG
        head.push(self.numbits);
        vbyte(&mut head, self.numentries);
        out.extend_from_slice(&head);
        out.push(crc8(&head));
        out.extend_from_slice(&self.data);
        out.extend_from_slice(&crc32c(&self.data).to_le_bytes());
    }
}

// ---------------------------------------------------------------------------
// BitSequence375  —  libhdt/src/bitsequence/BitSequence375.cpp:179-201
// ---------------------------------------------------------------------------

/// A plain bitmap, HDT's `BitSequence375`.
///
/// ```text
///     0x01 | vbyte(numbits) | crc8 | data | crc32c(LE)
/// ```
///
/// Bit `i` is byte `i/8`, bit position `i%8` (LSB-first) —
/// `libhdt/src/util/bitutil.h:69-81`. The rank/select index is **not**
/// serialized; `hdt-cpp` rebuilds it on load.
///
/// One quirk worth preserving: `numBytes(0) == 1`
/// (`BitSequence375.h:90-92`), so an empty bitmap still writes a single zero
/// byte. A zero-length data block there would shift everything after it.
#[derive(Default)]
pub(crate) struct BitSequence {
    numbits: u64,
    data: Vec<u8>,
}

impl BitSequence {
    pub(crate) fn with_capacity(bits: usize) -> Self {
        Self {
            numbits: 0,
            data: Vec::with_capacity(bits.div_ceil(8)),
        }
    }

    /// Append one bit.
    pub(crate) fn push(&mut self, bit: bool) {
        let idx = (self.numbits / 8) as usize;
        if idx >= self.data.len() {
            self.data.push(0);
        }
        if bit {
            self.data[idx] |= 1 << (self.numbits % 8);
        }
        self.numbits += 1;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> u64 {
        self.numbits
    }

    /// Read one bit back — for tests that decode what they just encoded.
    #[cfg(test)]
    pub(crate) fn get(&self, i: u64) -> bool {
        self.data[(i / 8) as usize] >> (i % 8) & 1 == 1
    }

    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        let mut head = Vec::with_capacity(11);
        head.push(0x01u8); // TYPE_BITMAP_PLAIN
        vbyte(&mut head, self.numbits);
        out.extend_from_slice(&head);
        out.push(crc8(&head));
        // numBytes(0) == 1: an empty bitmap is one zero byte, not nothing.
        if self.data.is_empty() {
            let z = [0u8];
            out.extend_from_slice(&z);
            out.extend_from_slice(&crc32c(&z).to_le_bytes());
        } else {
            out.extend_from_slice(&self.data);
            out.extend_from_slice(&crc32c(&self.data).to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked examples from the spec, verified against the real
    /// `dblp-20170124.hdt`: the shared section header at offset 0x74B is
    /// literally `02 | 6b 51 ee | 10 6b 40 89 | 90 | 25`.
    #[test]
    fn vbyte_matches_the_reference_file() {
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x80]),
            (5, &[0x85]),
            (16, &[0x90]),
            (127, &[0xFF]),
            (128, &[0x00, 0x81]),
            (300, &[0x2C, 0x82]),
            (1_812_715, &[0x6B, 0x51, 0xEE]),
            (19_936_656, &[0x10, 0x6B, 0x40, 0x89]),
            (88_150_324, &[0x34, 0x22, 0x04, 0xAA]),
        ];
        for (value, want) in cases {
            let mut got = Vec::new();
            vbyte(&mut got, *value);
            assert_eq!(&got[..], *want, "vbyte({value})");
        }
    }

    #[test]
    fn vbyte_marks_the_last_byte_not_the_continuations() {
        // The distinguishing property, stated as a test so the convention cannot
        // be "corrected" into the usual one.
        let mut got = Vec::new();
        vbyte(&mut got, 1_812_715);
        assert!(
            got[..got.len() - 1].iter().all(|b| b & 0x80 == 0),
            "every byte but the last must have the high bit CLEAR: {got:02x?}"
        );
        assert!(
            got[got.len() - 1] & 0x80 != 0,
            "the last byte must have the high bit SET: {got:02x?}"
        );
    }

    #[test]
    fn crc8_is_smbus() {
        // CRC-8/SMBUS check value: "123456789" -> 0xF4.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0x00);
    }

    #[test]
    fn crc16_is_arc() {
        // CRC-16/ARC check value: "123456789" -> 0xBB3D.
        assert_eq!(crc16(b"123456789"), 0xBB3D);
        assert_eq!(crc16(&[]), 0x0000);
    }

    #[test]
    fn crc32_is_castagnoli_not_zlib() {
        // CRC-32C check value: "123456789" -> 0xE3069283.
        // (zlib CRC-32 would give 0xCBF43926 — a different number, and a file
        // hdt-cpp rejects.)
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_ne!(crc32c(b"123456789"), 0xCBF4_3926);
    }

    /// The global control information of the real reference file, byte for byte.
    #[test]
    fn global_control_info_matches_the_reference_file() {
        let mut out = Vec::new();
        control_info(&mut out, CiType::Global, GLOBAL_FORMAT, &[]);
        let mut want = Vec::new();
        want.extend_from_slice(b"$HDT");
        want.push(1);
        want.extend_from_slice(GLOBAL_FORMAT.as_bytes());
        want.push(0);
        want.push(0);
        // dblp-20170124.hdt carries crc16 0x3576 here (little-endian 76 35).
        want.extend_from_slice(&0x3576u16.to_le_bytes());
        assert_eq!(out, want, "got {:02x?}", out);
        assert_eq!(out.len(), 0x28, "the header CI starts at 0x28");
    }

    /// The triples control information carries `order=1;` and nothing else —
    /// notably NOT `numTriples`, which only appears in the header N-Triples and
    /// in an index's control information.
    #[test]
    fn triples_control_info_matches_the_reference_file() {
        let mut out = Vec::new();
        control_info(
            &mut out,
            CiType::Triples,
            TRIPLES_FORMAT,
            &[("order", ORDER_SPO.to_string())],
        );
        assert!(out.ends_with(&0xe959u16.to_le_bytes()), "got {:02x?}", out);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("order=1;"));
        assert!(!text.contains("numTriples"));
    }

    #[test]
    fn bits_for_counts_the_high_bit() {
        assert_eq!(bits_for(0), 0);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(16), 5);
        assert_eq!(bits_for(19_936_656), 25); // dblp shared blocks
        assert_eq!(bits_for(u32::MAX as u64), 32);
    }

    #[test]
    fn log_sequence_packs_lsb_first() {
        // Three 5-bit values: 1, 2, 3 -> bits 00001 00010 00011 packed LSB-first
        // gives 0x41 0x0C 0x00... check by unpacking.
        let seq = LogSequence::new(&[1, 2, 3]);
        assert_eq!(seq.numbits, 2, "max is 3, which needs 2 bits");
        let seq = LogSequence::with_bits(&[1, 2, 3], 5);
        assert_eq!(seq.data.len(), (15u64).div_ceil(8) as usize);
        let get = |i: usize| -> u64 {
            let mut v = 0u64;
            for b in 0..5u64 {
                let pos = i as u64 * 5 + b;
                if seq.data[(pos / 8) as usize] >> (pos % 8) & 1 == 1 {
                    v |= 1 << b;
                }
            }
            v
        };
        assert_eq!((get(0), get(1), get(2)), (1, 2, 3));
    }

    #[test]
    fn log_sequence_of_nothing_is_well_formed() {
        let seq = LogSequence::new(&[]);
        let mut out = Vec::new();
        seq.write(&mut out);
        // type, numbits=0, vbyte(0)=0x80, crc8, no data, crc32 of nothing.
        assert_eq!(out[0], 0x01);
        assert_eq!(out[1], 0x00);
        assert_eq!(out[2], 0x80);
        // type + numbits + vbyte(0) + crc8 + (no data) + crc32
        assert_eq!(out.len(), 3 + 1 + 4);
    }

    #[test]
    fn bit_sequence_is_lsb_first_and_never_empty_on_disk() {
        let mut b = BitSequence::default();
        for bit in [true, false, false, true] {
            b.push(bit);
        }
        assert_eq!(b.len(), 4);
        assert_eq!(b.data[0], 0b1001);

        // The quirk: zero bits still writes one data byte.
        let empty = BitSequence::default();
        let mut out = Vec::new();
        empty.write(&mut out);
        // type(1) + vbyte(0)(1) + crc8(1) + data(1) + crc32(4)
        assert_eq!(out.len(), 8, "got {:02x?}", out);
    }

    #[test]
    fn bit_sequence_spans_byte_boundaries() {
        let mut b = BitSequence::default();
        for i in 0..20 {
            b.push(i % 3 == 0);
        }
        assert_eq!(b.len(), 20);
        assert_eq!(b.data.len(), 3);
        for i in 0..20usize {
            let got = b.data[i / 8] >> (i % 8) & 1 == 1;
            assert_eq!(got, i % 3 == 0, "bit {i}");
        }
    }
}
