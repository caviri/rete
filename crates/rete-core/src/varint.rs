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
/// Returns `None` on truncation or overflow (> 10 bytes).
pub fn read_uvarint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &byte) in buf.iter().enumerate() {
        if shift >= 64 {
            return None; // overflow
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
}
