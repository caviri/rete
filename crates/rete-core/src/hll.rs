//! A small HyperLogLog (2^14 registers, 16 KiB, ~0.8 % relative error), shared by `rete estimate`
//! and the memory-bounded build's early term-count refusal ([`crate::extbuild`]).

/// HyperLogLog cardinality estimator (Flajolet et al.), 2^P registers.
pub struct Hll {
    reg: Vec<u8>,
}

const P: u32 = 14;
const M: usize = 1 << P;

impl Default for Hll {
    fn default() -> Self {
        Self::new()
    }
}

impl Hll {
    /// An empty estimator.
    pub fn new() -> Self {
        Hll { reg: vec![0u8; M] }
    }

    /// Observe one term.
    pub fn add(&mut self, s: &str) {
        let h = fxhash64(s.as_bytes());
        let idx = (h >> (64 - P)) as usize;
        // rank = position of the first 1 bit in the remaining bits (1-based)
        let w = (h << P) | (1 << (P - 1));
        let rank = (w.leading_zeros() + 1) as u8;
        if rank > self.reg[idx] {
            self.reg[idx] = rank;
        }
    }

    /// The estimated number of distinct terms observed.
    pub fn count(&self) -> u64 {
        let m = M as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self.reg.iter().map(|&r| 2f64.powi(-(r as i32))).sum();
        let raw = alpha * m * m / sum;
        let zeros = self.reg.iter().filter(|&&r| r == 0).count();
        // small-range correction: linear counting while registers are sparse
        if raw <= 2.5 * m && zeros > 0 {
            (m * (m / zeros as f64).ln()).round() as u64
        } else {
            raw.round() as u64
        }
    }
}

/// A fast, allocation-free 64-bit string hash (FxHash-style, good enough for HLL).
fn fxhash64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    // final avalanche so the leading bits are well mixed (HLL reads the top P)
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h
}
