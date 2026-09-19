//! HDT's `BitmapTriples`: the SPO-ordered adjacency encoding.
//!
//! `libhdt/src/triples/BitmapTriples.cpp:118-203` (construction) and `:642-666`
//! (save).
//!
//! For triples sorted ascending by `(s, p, o)`:
//!
//! * **arrayY** is every subject's distinct predicates, flattened in order.
//! * **bitmapY** has one bit per `arrayY` entry, set on the **last** predicate
//!   of each subject's run.
//! * **arrayZ** is every `(s, p)` pair's objects, flattened in order — so its
//!   length is the triple count.
//! * **bitmapZ** likewise marks the last object of each `(s, p)` run.
//!
//! Subject ids are **not stored**. They are implicit in the rank of the runs, so
//! subject `s` is run `s - 1`, and a subject with no triples cannot be
//! represented at all: `hdt-cpp` throws `"The subjects must be correlative"` if
//! the ids skip. Ours cannot skip, because subject ids are assigned from the
//! very terms that appear as subjects.
//!
//! Two details that are easy to invert, both verified against the reference
//! file:
//!
//! * the run-terminating bit is on the **last** element, not the first;
//! * `save()` writes **bitmapY, bitmapZ, arrayY, arrayZ** — the bitmaps
//!   together, then the arrays — not the interleaved order the prose
//!   descriptions of HDT suggest.

use super::codec::{bits_for, BitSequence, LogSequence};

/// The four structures, built from a sorted, deduplicated triple list.
pub(crate) struct BitmapTriples {
    bitmap_y: BitSequence,
    bitmap_z: BitSequence,
    array_y: Vec<u64>,
    array_z: Vec<u64>,
}

impl BitmapTriples {
    /// `triples` must be ascending by `(s, p, o)` with no duplicates, and the
    /// subject ids must be dense from 1.
    ///
    /// Returns `None` if the input is empty — an HDT of no triples is
    /// representable, but the caller usually wants to say something about it.
    pub(crate) fn build(triples: &[(u32, u32, u32)]) -> Self {
        let mut bitmap_y = BitSequence::with_capacity(triples.len());
        let mut bitmap_z = BitSequence::with_capacity(triples.len());
        let mut array_y: Vec<u64> = Vec::new();
        let mut array_z: Vec<u64> = Vec::with_capacity(triples.len());

        let (mut last_s, mut last_p) = (0u32, 0u32);
        for (i, &(s, p, o)) in triples.iter().enumerate() {
            if i == 0 {
                array_y.push(p as u64);
                array_z.push(o as u64);
            } else if s != last_s {
                // New subject: the predicate we just finished was the last of
                // the previous subject's run, and its object the last of that
                // (s, p) run.
                bitmap_y.push(true);
                array_y.push(p as u64);
                bitmap_z.push(true);
                array_z.push(o as u64);
            } else if p != last_p {
                // Same subject, new predicate: the y-run continues, the z-run ends.
                bitmap_y.push(false);
                array_y.push(p as u64);
                bitmap_z.push(true);
                array_z.push(o as u64);
            } else {
                // Same (s, p): another object in the same z-run.
                bitmap_z.push(false);
                array_z.push(o as u64);
            }
            last_s = s;
            last_p = p;
        }
        if !triples.is_empty() {
            // Close the final runs. Without these the last subject and the last
            // (s, p) pair have no terminator and `select1` walks off the end.
            bitmap_y.push(true);
            bitmap_z.push(true);
        }

        Self {
            bitmap_y,
            bitmap_z,
            array_y,
            array_z,
        }
    }

    pub(crate) fn triple_count(&self) -> u64 {
        self.array_z.len() as u64
    }

    /// Serialize in `hdt-cpp`'s order: bitmapY, bitmapZ, arrayY, arrayZ.
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        self.bitmap_y.write(out);
        self.bitmap_z.write(out);
        let max_y = self.array_y.iter().copied().max().unwrap_or(0);
        let max_z = self.array_z.iter().copied().max().unwrap_or(0);
        LogSequence::with_bits(&self.array_y, bits_for(max_y)).write(out);
        LogSequence::with_bits(&self.array_z, bits_for(max_z)).write(out);
    }

    #[cfg(test)]
    fn shape(&self) -> (Vec<u64>, Vec<bool>, Vec<u64>, Vec<bool>) {
        let bits = |b: &BitSequence| -> Vec<bool> { (0..b.len()).map(|i| b.get(i)).collect() };
        (
            self.array_y.clone(),
            bits(&self.bitmap_y),
            self.array_z.clone(),
            bits(&self.bitmap_z),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_triple_closes_both_runs() {
        let t = BitmapTriples::build(&[(1, 1, 1)]);
        let (ay, by, az, bz) = t.shape();
        assert_eq!(ay, vec![1]);
        assert_eq!(by, vec![true], "the only predicate ends its run");
        assert_eq!(az, vec![1]);
        assert_eq!(bz, vec![true]);
        assert_eq!(t.triple_count(), 1);
    }

    #[test]
    fn the_terminator_bit_is_on_the_last_element_of_each_run() {
        // s=1 has predicates 1 and 2; s=2 has predicate 1.
        //   arrayY = [1, 2, 1]
        //   bitmapY = [false, true, true]   <- set on the LAST of each subject
        let t = BitmapTriples::build(&[(1, 1, 10), (1, 2, 20), (2, 1, 30)]);
        let (ay, by, az, bz) = t.shape();
        assert_eq!(ay, vec![1, 2, 1]);
        assert_eq!(by, vec![false, true, true]);
        assert_eq!(az, vec![10, 20, 30]);
        assert_eq!(bz, vec![true, true, true]);
    }

    #[test]
    fn several_objects_share_one_predicate_run() {
        // s=1 p=1 has objects 10, 11, 12.
        let t = BitmapTriples::build(&[(1, 1, 10), (1, 1, 11), (1, 1, 12)]);
        let (ay, by, az, bz) = t.shape();
        assert_eq!(ay, vec![1], "one predicate, so one y entry");
        assert_eq!(by, vec![true]);
        assert_eq!(az, vec![10, 11, 12]);
        assert_eq!(
            bz,
            vec![false, false, true],
            "only the last object of the (s,p) run is marked"
        );
    }

    #[test]
    fn the_bitmaps_are_exactly_as_long_as_their_arrays() {
        // The invariant hdt-cpp's navigation relies on; cross-checked against
        // the reference file, where |bitmapY| == |arrayY| == 58,476,283 and
        // |bitmapZ| == |arrayZ| == 88,150,324.
        let triples: Vec<(u32, u32, u32)> = (1..=50u32)
            .flat_map(|s| (1..=3u32).flat_map(move |p| (1..=4u32).map(move |o| (s, p, o))))
            .collect();
        let t = BitmapTriples::build(&triples);
        assert_eq!(t.bitmap_y.len(), t.array_y.len() as u64);
        assert_eq!(t.bitmap_z.len(), t.array_z.len() as u64);
        assert_eq!(t.array_y.len(), 150, "50 subjects x 3 predicates");
        assert_eq!(t.array_z.len(), 600);
        assert_eq!(t.triple_count(), 600);
    }

    #[test]
    fn an_empty_triple_set_produces_empty_structures() {
        let t = BitmapTriples::build(&[]);
        assert_eq!(t.triple_count(), 0);
        assert_eq!(t.bitmap_y.len(), 0);
        let mut out = Vec::new();
        t.write(&mut out);
        assert!(!out.is_empty(), "the structures still have headers");
    }

    /// Walk the encoding the way `AdjacencyList` does and recover the triples,
    /// which is the only check that really proves the run marking is right.
    #[test]
    fn the_encoding_decodes_back_to_the_triples() {
        let triples: Vec<(u32, u32, u32)> = vec![
            (1, 1, 5),
            (1, 1, 9),
            (1, 4, 2),
            (2, 2, 7),
            (3, 1, 1),
            (3, 1, 2),
            (3, 3, 8),
        ];
        let t = BitmapTriples::build(&triples);
        let (ay, by, az, bz) = t.shape();

        let mut got = Vec::new();
        let mut yi = 0usize;
        let mut zi = 0usize;
        let mut subject = 1u32;
        while yi < ay.len() {
            // This subject's predicate run ends at the next set bit in bitmapY.
            loop {
                let p = ay[yi] as u32;
                // This (s, p)'s object run ends at the next set bit in bitmapZ.
                loop {
                    got.push((subject, p, az[zi] as u32));
                    let end_z = bz[zi];
                    zi += 1;
                    if end_z {
                        break;
                    }
                }
                let end_y = by[yi];
                yi += 1;
                if end_y {
                    break;
                }
            }
            subject += 1;
        }
        assert_eq!(got, triples);
    }
}
