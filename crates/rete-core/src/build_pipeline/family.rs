use crate::index::Tile;
use crate::triples::{encode_sorted_unique, encoded_sorted_unique_len, TripleBlock};
use crate::varint::uvarint_len;
use crate::Triple;

use super::spool::TripleSpool;
use super::BuildPipelineError;

/// The four non-leading zone bounds retained for each physical order.
pub(crate) type Synopsis = (u32, u32, u32, u32);

/// A pair of orders that share their leading canonical component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexFamily {
    Subject,
    Predicate,
    Object,
}

impl IndexFamily {
    pub(crate) const fn slot(self) -> usize {
        match self {
            Self::Subject => 0,
            Self::Predicate => 1,
            Self::Object => 2,
        }
    }

    fn first(self, (subject, predicate, object): Triple) -> Triple {
        match self {
            Self::Subject => (subject, predicate, object),
            Self::Predicate => (predicate, object, subject),
            Self::Object => (object, subject, predicate),
        }
    }

    fn second(self, (subject, predicate, object): Triple) -> Triple {
        match self {
            Self::Subject => (subject, object, predicate),
            Self::Predicate => (predicate, subject, object),
            Self::Object => (object, predicate, subject),
        }
    }
}

/// One common leading-id range with independently encoded sibling orders.
pub(crate) struct PairedTile {
    pub min_a: u32,
    pub max_a: u32,
    pub first: Vec<u8>,
    pub second: Vec<u8>,
    pub first_synopsis: Synopsis,
    pub second_synopsis: Synopsis,
}

/// The finished physical representation of one leading-component family.
pub(crate) struct FamilyIndex {
    pub family: IndexFamily,
    pub tiles: Vec<PairedTile>,
}

/// Borrowed views of a family's two logical sections.
pub(crate) struct FamilyView<'a> {
    pub family: IndexFamily,
    pub first: &'a [Tile],
    pub second: &'a [Tile],
}

fn overflow(what: &'static str) -> BuildPipelineError {
    BuildPipelineError::Overflow(what)
}

fn radix_pass(
    input: &mut Vec<Triple>,
    scratch: &mut Vec<Triple>,
    byte: impl Fn(Triple) -> u8,
) -> Result<(), BuildPipelineError> {
    let mut counts = [0usize; 256];
    for &triple in input.iter() {
        let bucket = usize::from(byte(triple));
        counts[bucket] = counts[bucket]
            .checked_add(1)
            .ok_or_else(|| overflow("radix bucket count"))?;
    }
    let mut offsets = [0usize; 256];
    let mut next = 0usize;
    for (offset, count) in offsets.iter_mut().zip(counts) {
        *offset = next;
        next = next
            .checked_add(count)
            .ok_or_else(|| overflow("radix prefix offset"))?;
    }
    if next != input.len() {
        return Err(BuildPipelineError::InvalidSpool("radix input length"));
    }
    scratch.clear();
    scratch.resize(input.len(), (0, 0, 0));
    for &triple in input.iter() {
        let bucket = usize::from(byte(triple));
        let slot = offsets[bucket];
        scratch[slot] = triple;
        offsets[bucket] = slot
            .checked_add(1)
            .ok_or_else(|| overflow("radix scatter offset"))?;
    }
    std::mem::swap(input, scratch);
    Ok(())
}

fn radix_pass_slice(
    input: &mut [Triple],
    scratch: &mut Vec<Triple>,
    byte: impl Fn(Triple) -> u8,
) -> Result<(), BuildPipelineError> {
    let mut counts = [0usize; 256];
    for &triple in input.iter() {
        let bucket = usize::from(byte(triple));
        counts[bucket] = counts[bucket]
            .checked_add(1)
            .ok_or_else(|| overflow("radix bucket count"))?;
    }
    let mut offsets = [0usize; 256];
    let mut next = 0usize;
    for (offset, count) in offsets.iter_mut().zip(counts) {
        *offset = next;
        next = next
            .checked_add(count)
            .ok_or_else(|| overflow("radix prefix offset"))?;
    }
    if next != input.len() {
        return Err(BuildPipelineError::InvalidSpool("radix input length"));
    }
    scratch.clear();
    scratch.resize(input.len(), (0, 0, 0));
    for &triple in input.iter() {
        let bucket = usize::from(byte(triple));
        let slot = offsets[bucket];
        scratch[slot] = triple;
        offsets[bucket] = slot
            .checked_add(1)
            .ok_or_else(|| overflow("radix scatter offset"))?;
    }
    input.copy_from_slice(scratch);
    Ok(())
}

/// Stable, safe LSD radix sorting for selected tuple components. Components
/// are supplied from least to most significant key (e.g. `[2, 1, 0]`).
fn radix_sort(input: &mut Vec<Triple>, components: &[usize]) -> Result<(), BuildPipelineError> {
    let mut scratch = Vec::new();
    for &component in components {
        if component >= 3 {
            return Err(BuildPipelineError::InvalidSpool("radix component"));
        }
        for shift in [0u32, 8, 16, 24] {
            radix_pass(input, &mut scratch, |triple| {
                (([triple.0, triple.1, triple.2][component] >> shift) & 0xff) as u8
            })?;
        }
    }
    Ok(())
}

fn radix_sort_slice(
    input: &mut [Triple],
    components: &[usize],
    scratch: &mut Vec<Triple>,
) -> Result<(), BuildPipelineError> {
    for &component in components {
        if component >= 3 {
            return Err(BuildPipelineError::InvalidSpool("radix component"));
        }
        for shift in [0u32, 8, 16, 24] {
            radix_pass_slice(input, scratch, |triple| {
                (([triple.0, triple.1, triple.2][component] >> shift) & 0xff) as u8
            })?;
        }
    }
    Ok(())
}

fn collect_spool(spool: &TripleSpool) -> Result<Vec<Triple>, BuildPipelineError> {
    let mut triples = Vec::new();
    spool.for_each_block(1 << 16, &mut |block| {
        triples
            .try_reserve(block.len())
            .map_err(|_| overflow("family spool collection"))?;
        triples.extend_from_slice(block);
        Ok(())
    })?;
    Ok(triples)
}

#[derive(Clone, Copy)]
struct GroupSummary {
    a: u32,
    min_b: u32,
    max_b: u32,
    min_c: u32,
    max_c: u32,
    count: u64,
    /// Everything after the leading-ID delta in one encoded a-group.
    body_without_a: usize,
}

impl GroupSummary {
    fn from_sorted(group: &[Triple]) -> Result<Self, BuildPipelineError> {
        let &(a, first_b, first_c) = group
            .first()
            .ok_or(BuildPipelineError::InvalidSpool("empty leading group"))?;
        let mut summary = Self {
            a,
            min_b: first_b,
            max_b: first_b,
            min_c: first_c,
            max_c: first_c,
            count: 0,
            body_without_a: 0,
        };
        let mut i = 0usize;
        let mut previous_b = 0u32;
        let mut group_count = 0u64;
        while i < group.len() {
            let (current_a, b, _) = group[i];
            if current_a != a {
                return Err(BuildPipelineError::InvalidSpool("mixed leading group"));
            }
            summary.min_b = summary.min_b.min(b);
            summary.max_b = summary.max_b.max(b);
            summary.body_without_a = summary
                .body_without_a
                .checked_add(uvarint_len((b - previous_b) as u64))
                .ok_or_else(|| overflow("family secondary delta"))?;
            previous_b = b;
            let mut previous_c = 0u32;
            let mut c_count = 0u64;
            while i < group.len() && group[i].0 == a && group[i].1 == b {
                let c = group[i].2;
                summary.min_c = summary.min_c.min(c);
                summary.max_c = summary.max_c.max(c);
                summary.body_without_a = summary
                    .body_without_a
                    .checked_add(uvarint_len((c - previous_c) as u64))
                    .ok_or_else(|| overflow("family tertiary delta"))?;
                previous_c = c;
                c_count = c_count
                    .checked_add(1)
                    .ok_or_else(|| overflow("family tertiary count"))?;
                summary.count = summary
                    .count
                    .checked_add(1)
                    .ok_or_else(|| overflow("family triple count"))?;
                i += 1;
            }
            summary.body_without_a = summary
                .body_without_a
                .checked_add(uvarint_len(c_count))
                .ok_or_else(|| overflow("family tertiary count"))?;
            group_count = group_count
                .checked_add(1)
                .ok_or_else(|| overflow("family secondary count"))?;
        }
        summary.body_without_a = summary
            .body_without_a
            .checked_add(uvarint_len(group_count))
            .ok_or_else(|| overflow("family secondary count"))?;
        Ok(summary)
    }
}

#[derive(Clone, Copy, Default)]
struct TileSummary {
    min_a: u32,
    max_a: u32,
    min_b: u32,
    max_b: u32,
    min_c: u32,
    max_c: u32,
    count: u64,
    groups: u64,
    body: usize,
    empty: bool,
}

impl TileSummary {
    fn empty() -> Self {
        Self {
            empty: true,
            ..Self::default()
        }
    }

    fn with_group(self, group: GroupSummary) -> Result<Self, BuildPipelineError> {
        let mut next = self;
        if next.empty {
            next.min_a = group.a;
            next.max_a = group.a;
            next.min_b = group.min_b;
            next.max_b = group.max_b;
            next.min_c = group.min_c;
            next.max_c = group.max_c;
            next.empty = false;
        } else {
            if group.a <= next.max_a {
                return Err(BuildPipelineError::InvalidSpool(
                    "nonascending leading groups",
                ));
            }
            next.max_a = group.a;
            next.min_b = next.min_b.min(group.min_b);
            next.max_b = next.max_b.max(group.max_b);
            next.min_c = next.min_c.min(group.min_c);
            next.max_c = next.max_c.max(group.max_c);
        }
        let previous_a = if self.empty { 0 } else { self.max_a };
        next.body = next
            .body
            .checked_add(uvarint_len((group.a - previous_a) as u64))
            .and_then(|size| size.checked_add(group.body_without_a))
            .ok_or_else(|| overflow("family group body"))?;
        next.count = next
            .count
            .checked_add(group.count)
            .ok_or_else(|| overflow("family tile count"))?;
        next.groups = next
            .groups
            .checked_add(1)
            .ok_or_else(|| overflow("family leading count"))?;
        Ok(next)
    }

    fn encoded_size(self) -> Result<usize, BuildPipelineError> {
        if self.empty {
            return Ok(8);
        }
        [
            uvarint_len(self.min_a as u64),
            uvarint_len(self.max_a as u64),
            uvarint_len(self.min_b as u64),
            uvarint_len(self.max_b as u64),
            uvarint_len(self.min_c as u64),
            uvarint_len(self.max_c as u64),
            uvarint_len(self.count),
            uvarint_len(self.groups),
            self.body,
        ]
        .into_iter()
        .try_fold(0usize, |total, part| total.checked_add(part))
        .ok_or_else(|| overflow("family tile size"))
    }
}

fn group_ranges(sorted: &[Triple]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < sorted.len() {
        let a = sorted[start].0;
        let mut end = start + 1;
        while end < sorted.len() && sorted[end].0 == a {
            end += 1;
        }
        ranges.push(start..end);
        start = end;
    }
    ranges
}

fn split_group(
    sorted: &[Triple],
    budget: usize,
) -> Result<Vec<std::ops::Range<usize>>, BuildPipelineError> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < sorted.len() {
        let single = encoded_sorted_unique_len(&sorted[start..start + 1])
            .map_err(|_| BuildPipelineError::InvalidSpool("invalid family continuation"))?;
        if single > budget {
            return Err(BuildPipelineError::InvalidSpool(
                "tile budget smaller than one encoded triple",
            ));
        }
        let mut low = start + 1;
        let mut high = sorted.len();
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            let size = encoded_sorted_unique_len(&sorted[start..middle])
                .map_err(|_| BuildPipelineError::InvalidSpool("invalid family continuation"))?;
            if size <= budget {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        ranges.push(start..low);
        start = low;
    }
    Ok(ranges)
}

fn align_slices(
    sorted: &[Triple],
    ranges: &mut Vec<std::ops::Range<usize>>,
    wanted: usize,
    budget: usize,
) -> Result<(), BuildPipelineError> {
    while ranges.len() < wanted {
        let (index, range) = ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| range.len() > 1)
            .max_by_key(|(_, range)| range.len())
            .map(|(index, range)| (index, range.clone()))
            .ok_or(BuildPipelineError::InvalidSpool(
                "unalignable family continuation",
            ))?;
        let middle = range.start + range.len() / 2;
        for slice in [&sorted[range.start..middle], &sorted[middle..range.end]] {
            let size = encoded_sorted_unique_len(slice)
                .map_err(|_| BuildPipelineError::InvalidSpool("invalid family continuation"))?;
            if size > budget {
                return Err(BuildPipelineError::InvalidSpool(
                    "oversize family continuation",
                ));
            }
        }
        ranges.splice(index..=index, [range.start..middle, middle..range.end]);
    }
    Ok(())
}

fn encode_pair(
    first: &[Triple],
    second: &[Triple],
    budget: usize,
) -> Result<PairedTile, BuildPipelineError> {
    let first_expected = encoded_sorted_unique_len(first)
        .map_err(|_| BuildPipelineError::InvalidSpool("invalid first family order"))?;
    let second_expected = encoded_sorted_unique_len(second)
        .map_err(|_| BuildPipelineError::InvalidSpool("invalid second family order"))?;
    if first_expected > budget || second_expected > budget {
        return Err(BuildPipelineError::InvalidSpool(
            "family tile exceeds budget",
        ));
    }
    let first_bytes = encode_sorted_unique(first);
    let second_bytes = encode_sorted_unique(second);
    if first_bytes.len() != first_expected || second_bytes.len() != second_expected {
        return Err(BuildPipelineError::InvalidSpool(
            "family tile accounting mismatch",
        ));
    }
    let first_block = TripleBlock::parse(&first_bytes)
        .map_err(|_| BuildPipelineError::InvalidSpool("first family tile does not parse"))?;
    let second_block = TripleBlock::parse(&second_bytes)
        .map_err(|_| BuildPipelineError::InvalidSpool("second family tile does not parse"))?;
    let first_zone = first_block.zone();
    let second_zone = second_block.zone();
    let first_synopsis = (
        first_zone.min_b,
        first_zone.max_b,
        first_zone.min_c,
        first_zone.max_c,
    );
    let second_synopsis = (
        second_zone.min_b,
        second_zone.max_b,
        second_zone.min_c,
        second_zone.max_c,
    );
    if first_zone.min_a != second_zone.min_a || first_zone.max_a != second_zone.max_a {
        return Err(BuildPipelineError::InvalidSpool(
            "family sibling ranges differ",
        ));
    }
    Ok(PairedTile {
        min_a: first_zone.min_a,
        max_a: first_zone.max_a,
        first: first_bytes,
        second: second_bytes,
        first_synopsis,
        second_synopsis,
    })
}

/// Construct both physical orders for one leading component from one replay of
/// a canonical spool. Full leading groups stay together; only oversize groups
/// become aligned continuation pairs.
pub(crate) fn build_family(
    spool: &TripleSpool,
    family: IndexFamily,
    tile_budget: usize,
) -> Result<FamilyIndex, BuildPipelineError> {
    if tile_budget == 0 {
        return Err(BuildPipelineError::InvalidSpool("zero family tile budget"));
    }
    let source = collect_spool(spool)?;
    let mut first = Vec::new();
    first
        .try_reserve(source.len())
        .map_err(|_| overflow("first family order"))?;
    first.extend(source.into_iter().map(|triple| family.first(triple)));
    radix_sort(&mut first, &[2, 1, 0])?;
    first.dedup();
    let mut second = Vec::new();
    second
        .try_reserve(first.len())
        .map_err(|_| overflow("second family order"))?;
    let first_groups = group_ranges(&first);
    second.extend(first.iter().copied().map(|(a, x, y)| (a, y, x)));
    // Tail-only sorts save four leading-key passes per hot group. For graphs
    // dominated by tiny groups, their fixed 256-bucket setup costs more than a
    // single full sibling pass, so retain the same deterministic radix order
    // without paying that per-group overhead.
    if first_groups.len() > first.len() / 8 {
        radix_sort(&mut second, &[2, 1, 0])?;
    } else {
        let mut tail_scratch = Vec::new();
        for group in &first_groups {
            if group.len() > 1 {
                // `first` is already partitioned by leading id. Sorting only
                // this group's tail keys retains that partition.
                radix_sort_slice(&mut second[group.clone()], &[2, 1], &mut tail_scratch)?;
            }
        }
    }
    let second_groups = group_ranges(&second);
    if first_groups.len() != second_groups.len() {
        return Err(BuildPipelineError::InvalidSpool(
            "family leading groups differ",
        ));
    }
    let mut tiles = Vec::new();
    let mut current_first = Vec::new();
    let mut current_second = Vec::new();
    let mut first_summary = TileSummary::empty();
    let mut second_summary = TileSummary::empty();
    let flush = |tiles: &mut Vec<PairedTile>,
                 current_first: &mut Vec<Triple>,
                 current_second: &mut Vec<Triple>,
                 first_summary: &mut TileSummary,
                 second_summary: &mut TileSummary|
     -> Result<(), BuildPipelineError> {
        if !first_summary.empty {
            tiles.push(encode_pair(current_first, current_second, tile_budget)?);
            current_first.clear();
            current_second.clear();
            *first_summary = TileSummary::empty();
            *second_summary = TileSummary::empty();
        }
        Ok(())
    };

    for (first_range, second_range) in first_groups.into_iter().zip(second_groups) {
        let first_group = &first[first_range];
        let second_group = &second[second_range];
        if first_group.first().map(|triple| triple.0) != second_group.first().map(|triple| triple.0)
        {
            return Err(BuildPipelineError::InvalidSpool(
                "family leading id differs",
            ));
        }
        let first_group_summary = GroupSummary::from_sorted(first_group)?;
        let second_group_summary = GroupSummary::from_sorted(second_group)?;
        let next_first = first_summary.with_group(first_group_summary)?;
        let next_second = second_summary.with_group(second_group_summary)?;
        if next_first.encoded_size()? <= tile_budget && next_second.encoded_size()? <= tile_budget {
            current_first.extend_from_slice(first_group);
            current_second.extend_from_slice(second_group);
            first_summary = next_first;
            second_summary = next_second;
            continue;
        }
        if !first_summary.empty {
            flush(
                &mut tiles,
                &mut current_first,
                &mut current_second,
                &mut first_summary,
                &mut second_summary,
            )?;
            let single_first = first_summary.with_group(first_group_summary)?;
            let single_second = second_summary.with_group(second_group_summary)?;
            if single_first.encoded_size()? <= tile_budget
                && single_second.encoded_size()? <= tile_budget
            {
                current_first.extend_from_slice(first_group);
                current_second.extend_from_slice(second_group);
                first_summary = single_first;
                second_summary = single_second;
                continue;
            }
        }

        let mut first_slices = split_group(first_group, tile_budget)?;
        let mut second_slices = split_group(second_group, tile_budget)?;
        let target = first_slices.len().max(second_slices.len());
        align_slices(first_group, &mut first_slices, target, tile_budget)?;
        align_slices(second_group, &mut second_slices, target, tile_budget)?;
        for (first_slice, second_slice) in first_slices.into_iter().zip(second_slices) {
            tiles.push(encode_pair(
                &first_group[first_slice],
                &second_group[second_slice],
                tile_budget,
            )?);
        }
    }
    flush(
        &mut tiles,
        &mut current_first,
        &mut current_second,
        &mut first_summary,
        &mut second_summary,
    )?;
    Ok(FamilyIndex { family, tiles })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::build_pipeline::spool::BuildTemp;
    use crate::build_pipeline::spool::TripleSpool;
    use crate::triples::TripleBlock;
    use crate::Triple;

    use super::{build_family, IndexFamily};

    fn fixture() -> Vec<Triple> {
        vec![
            (2, 8, 4),
            (1, 9, 3),
            (1, 7, 5),
            (2, 6, 4),
            (1, 7, 2),
            (2, 8, 1),
            (1, 7, 2),
        ]
    }

    fn decode(bytes: &[Vec<u8>]) -> Vec<Triple> {
        bytes
            .iter()
            .flat_map(|tile| TripleBlock::parse(tile).unwrap().triples())
            .collect()
    }

    fn sorted_spo() -> Vec<Triple> {
        vec![
            (1, 7, 2),
            (1, 7, 5),
            (1, 9, 3),
            (2, 6, 4),
            (2, 8, 1),
            (2, 8, 4),
        ]
    }

    fn sorted_sop() -> Vec<Triple> {
        vec![
            (1, 2, 7),
            (1, 3, 9),
            (1, 5, 7),
            (2, 1, 8),
            (2, 4, 6),
            (2, 4, 8),
        ]
    }

    fn ranges(family: &super::FamilyIndex, first: bool) -> Vec<(u32, u32)> {
        family
            .tiles
            .iter()
            .map(|tile| {
                let bytes = if first { &tile.first } else { &tile.second };
                let block = TripleBlock::parse(bytes).unwrap();
                (block.zone().min_a, block.zone().max_a)
            })
            .collect()
    }

    fn images(family: &super::FamilyIndex, first: bool) -> Vec<Vec<u8>> {
        family
            .tiles
            .iter()
            .map(|tile| {
                if first {
                    tile.first.clone()
                } else {
                    tile.second.clone()
                }
            })
            .collect()
    }

    fn expected(mut triples: Vec<Triple>, family: IndexFamily, second: bool) -> Vec<Triple> {
        triples = triples
            .into_iter()
            .map(|(s, p, o)| match (family, second) {
                (IndexFamily::Subject, false) => (s, p, o),
                (IndexFamily::Subject, true) => (s, o, p),
                (IndexFamily::Predicate, false) => (p, o, s),
                (IndexFamily::Predicate, true) => (p, s, o),
                (IndexFamily::Object, false) => (o, s, p),
                (IndexFamily::Object, true) => (o, p, s),
            })
            .collect();
        triples.sort_unstable();
        triples.dedup();
        triples
    }

    fn assert_family(triples: Vec<Triple>, family: IndexFamily, budget: usize) {
        let built = build_family(&TripleSpool::Resident(triples.clone()), family, budget).unwrap();
        assert_eq!(
            decode(&images(&built, true)),
            expected(triples.clone(), family, false)
        );
        assert_eq!(
            decode(&images(&built, false)),
            expected(triples, family, true)
        );
        assert_eq!(ranges(&built, true), ranges(&built, false));
        assert!(built.tiles.iter().all(|tile| {
            tile.first.len() <= budget
                && tile.second.len() <= budget
                && TripleBlock::parse(&tile.first).is_ok()
                && TripleBlock::parse(&tile.second).is_ok()
        }));
    }

    #[test]
    fn subject_family_produces_spo_and_sop_with_shared_ranges() {
        let family =
            build_family(&TripleSpool::Resident(fixture()), IndexFamily::Subject, 64).unwrap();
        assert_eq!(
            decode(
                &family
                    .tiles
                    .iter()
                    .map(|tile| tile.first.clone())
                    .collect::<Vec<_>>(),
            ),
            sorted_spo()
        );
        assert_eq!(
            decode(
                &family
                    .tiles
                    .iter()
                    .map(|tile| tile.second.clone())
                    .collect::<Vec<_>>(),
            ),
            sorted_sop()
        );
        assert_eq!(ranges(&family, true), ranges(&family, false));
    }

    fn hot_subject(count: u32) -> Vec<Triple> {
        (0..count).map(|id| (7, id / 7, id)).collect()
    }

    #[test]
    fn mega_group_continuations_are_bounded_and_complete_in_both_orders() {
        let hot = hot_subject(20_000);
        let family = build_family(
            &TripleSpool::Resident(hot.clone()),
            IndexFamily::Subject,
            256,
        )
        .unwrap();
        assert!(family.tiles.len() > 1);
        let first: Vec<_> = family.tiles.iter().map(|tile| tile.first.clone()).collect();
        let second: Vec<_> = family
            .tiles
            .iter()
            .map(|tile| tile.second.clone())
            .collect();
        let mut spo = hot.clone();
        spo.sort_unstable();
        spo.dedup();
        let mut sop: Vec<_> = hot.into_iter().map(|(s, p, o)| (s, o, p)).collect();
        sop.sort_unstable();
        sop.dedup();
        assert_eq!(decode(&first), spo);
        assert_eq!(decode(&second), sop);
        assert!(family
            .tiles
            .iter()
            .all(|tile| tile.first.len() <= 256 && tile.second.len() <= 256));
        assert_eq!(ranges(&family, true), ranges(&family, false));
    }

    #[test]
    fn empty_singleton_duplicates_and_high_ids_are_deterministic() {
        for triples in [
            Vec::new(),
            vec![(u32::MAX, u32::MAX - 1, u32::MAX - 2)],
            vec![
                (u32::MAX, 1, 2),
                (u32::MAX, 1, 2),
                (0, u32::MAX, u32::MAX - 1),
                (1, 0, u32::MAX),
            ],
        ] {
            for family in [
                IndexFamily::Subject,
                IndexFamily::Predicate,
                IndexFamily::Object,
            ] {
                assert_family(triples.clone(), family, 64);
                let one =
                    build_family(&TripleSpool::Resident(triples.clone()), family, 64).unwrap();
                let two =
                    build_family(&TripleSpool::Resident(triples.clone()), family, 64).unwrap();
                assert_eq!(images(&one, true), images(&two, true));
                assert_eq!(images(&one, false), images(&two, false));
            }
        }
    }

    #[test]
    fn tiny_impossible_budget_is_a_clean_error() {
        let result = build_family(
            &TripleSpool::Resident(vec![(u32::MAX, u32::MAX, u32::MAX)]),
            IndexFamily::Subject,
            1,
        );
        assert!(matches!(
            result,
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(_))
        ));
    }

    #[test]
    fn file_spool_replay_matches_resident_for_every_family() {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "rete-family-spool-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let temp = BuildTemp::new(&parent).unwrap();
        let triples = fixture();
        let file = TripleSpool::write_file(&temp, "triples.tri", &triples).unwrap();
        for family in [
            IndexFamily::Subject,
            IndexFamily::Predicate,
            IndexFamily::Object,
        ] {
            let resident =
                build_family(&TripleSpool::Resident(triples.clone()), family, 64).unwrap();
            let replayed = build_family(&file, family, 64).unwrap();
            assert_eq!(images(&resident, true), images(&replayed, true));
            assert_eq!(images(&resident, false), images(&replayed, false));
        }
        drop(file);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    proptest! {
        #[test]
        fn radix_orders_match_sort_unstable_for_all_families(
            triples in prop::collection::vec((any::<u32>(), any::<u32>(), any::<u32>()), 0..80)
        ) {
            for family in [IndexFamily::Subject, IndexFamily::Predicate, IndexFamily::Object] {
                assert_family(triples.clone(), family, 96);
            }
        }
    }
}
