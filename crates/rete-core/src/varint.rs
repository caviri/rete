//! Unsigned LEB128 varints — the workhorse integer encoding used throughout the
//! dictionary and triple sections.

/// Append `value` to `out` as an unsigned LEB128 varint.
pub fn write_uvarint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Read an unsigned LEB128 varint from `buf`, returning `(value, bytes_read)`.
/// Returns `None` on truncation or overflow: more than 10 bytes, or a 10th
/// byte carrying more than the one bit a `u64` has left.
pub fn read_uvarint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        if shift >= 64 {
            return None; // overflow
        }
        // The 10th byte holds bit 63 only. Anything more would be shifted out
        // and lost, decoding a corrupt length as some smaller, plausible one.
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None // truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_samples() {
        for v in [0u64, 1, 127, 128, 300, 16_384, u32::MAX as u64, u64::MAX] {
            let mut b = Vec::new();
            write_uvarint(&mut b, v);
            let (got, n) = read_uvarint(&b).unwrap();
            assert_eq!(got, v);
            assert_eq!(n, b.len());
        }
    }

    #[test]
    fn truncated_is_none() {
        assert!(read_uvarint(&[0x80]).is_none());
    }

    #[test]
    fn tenth_byte_overflow_is_none() {
        // u64::MAX is nine 0xff bytes and a final 0x01.
        let mut max = [0xffu8; 10];
        max[9] = 0x01;
        assert_eq!(read_uvarint(&max), Some((u64::MAX, 10)));
        // Any other payload bit in the 10th byte does not fit a u64. Before,
        // those bits were shifted out and the value decoded as u64::MAX or as
        // a small number, depending on which bits were set.
        for last in [0x02u8, 0x03, 0x40, 0x7e, 0x7f] {
            let mut bad = [0xffu8; 10];
            bad[9] = last;
            assert_eq!(read_uvarint(&bad), None, "10th byte {last:#x}");
        }
        let mut small = [0x80u8; 10];
        small[9] = 0x02; // decoded as 0 before
        assert_eq!(read_uvarint(&small), None);
        // An 11th byte is still refused.
        assert_eq!(read_uvarint(&[0x80; 11]), None);
    }
}
