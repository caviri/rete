use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

use crate::index::Tile;
use crate::triples::{encode_sorted_unique, encoded_sorted_unique_len, TripleBlock};
use crate::varint::uvarint_len;
use crate::Triple;

use super::spool::{BuildTemp, TripleSpool};
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
    radix_sort_with_scratch(input, components, &mut scratch)
}

fn radix_sort_with_scratch(
    input: &mut Vec<Triple>,
    components: &[usize],
    scratch: &mut Vec<Triple>,
) -> Result<(), BuildPipelineError> {
    for &component in components {
        if component >= 3 {
            return Err(BuildPipelineError::InvalidSpool("radix component"));
        }
        for shift in [0u32, 8, 16, 24] {
            radix_pass(input, scratch, |triple| {
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

/// The only header term that can shrink while extending a sorted fixed-leading
/// segment is the width of `min_c`, from five uvarint bytes to one.
const MAX_APPEND_RECOVERY_BYTES: usize = 4;

type PairSlice = (std::ops::Range<usize>, std::ops::Range<usize>);

/// Synchronously segment sibling orders by rank. Every accepted boundary was
/// observed to fit in both orders; after either size exceeds the budget plus
/// the only possible future header recovery, it cannot become valid again.
fn synchronous_slices(
    first: &[Triple],
    second: &[Triple],
    budget: usize,
) -> Result<Vec<PairSlice>, BuildPipelineError> {
    if first.len() != second.len() {
        return Err(BuildPipelineError::InvalidSpool(
            "family sibling lengths differ",
        ));
    }
    if first.is_empty() {
        return Ok(Vec::new());
    }
    let recovery_limit = budget
        .checked_add(MAX_APPEND_RECOVERY_BYTES)
        .ok_or_else(|| overflow("family recovery budget"))?;
    let mut slices = Vec::new();
    let mut start = 0usize;
    while start < first.len() {
        let mut first_sizer = SegmentSizer::new(first[start].0);
        let mut second_sizer = SegmentSizer::new(second[start].0);
        let mut end = start;
        let mut last_valid = None;
        loop {
            if end == first.len() {
                break;
            }
            first_sizer.push(first[end])?;
            second_sizer.push(second[end])?;
            end = end
                .checked_add(1)
                .ok_or_else(|| overflow("family continuation end"))?;
            let first_size = first_sizer.encoded_size()?;
            let second_size = second_sizer.encoded_size()?;
            if first_size <= budget && second_size <= budget {
                last_valid = Some(end);
            }
            if first_size > recovery_limit || second_size > recovery_limit {
                break;
            }
        }
        let cut = last_valid.ok_or(BuildPipelineError::InvalidSpool(
            "family continuation has no common bounded prefix",
        ))?;
        slices.push((start..cut, start..cut));
        start = cut;
    }
    Ok(slices)
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

const FILE_RUN_FANIN: usize = 16;

/// A nonempty raw triple block has seven zone/count varints and six body
/// varints. Six arbitrary u32 extrema need five bytes each, the count and
/// three group counts need one byte each, and the three deltas need five bytes
/// each: `6 * 5 + 1 + 3 + 3 * 5 = 49` bytes. Below this, a singleton can be
/// temporarily unencodable even when a larger neighbor-sharing segment fits,
/// which has no bounded common-partition guarantee for the staged builder.
const MIN_FAMILY_TILE_BUDGET: usize = 49;

/// Explicit bounded working-set cap for file-backed family construction. Every
/// generated radix run holds no more than this many triples; it scales with the
/// requested tile budget rather than the spool's total statement count.
fn file_run_record_cap(tile_budget: usize) -> usize {
    (tile_budget / 12).clamp(1, 16 * 1024)
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct FileWorkingSet {
    records: usize,
    bytes: usize,
    descriptors: usize,
    max_single_vec_records: usize,
}

#[cfg(test)]
std::thread_local! {
    static FILE_PEAK_WORKING: std::cell::Cell<FileWorkingSet> = const { std::cell::Cell::new(FileWorkingSet { records: 0, bytes: 0, descriptors: 0, max_single_vec_records: 0 }) };
}

#[cfg(test)]
fn observe_file_live(
    triple_capacities: &[usize],
    additional_triple_capacities: &[usize],
    pair_capacities: &[usize],
    metadata_bytes: usize,
    io_buffer_bytes: usize,
    descriptors: usize,
) {
    let triple_records = triple_capacities
        .iter()
        .chain(additional_triple_capacities)
        .copied()
        .fold(0usize, usize::saturating_add);
    let pair_records = pair_capacities
        .iter()
        .copied()
        .fold(0usize, usize::saturating_add);
    let records = triple_records.saturating_add(pair_records.saturating_mul(2));
    let bytes = triple_records
        .saturating_mul(std::mem::size_of::<Triple>())
        .saturating_add(pair_records.saturating_mul(std::mem::size_of::<(Triple, Triple)>()))
        .saturating_add(metadata_bytes)
        .saturating_add(io_buffer_bytes);
    let max_single_vec_records = triple_capacities
        .iter()
        .chain(additional_triple_capacities)
        .copied()
        .chain(
            pair_capacities
                .iter()
                .copied()
                .map(|capacity| capacity.saturating_mul(2)),
        )
        .max()
        .unwrap_or(0);
    FILE_PEAK_WORKING.with(|peak| {
        let previous = peak.get();
        peak.set(FileWorkingSet {
            records: previous.records.max(records),
            bytes: previous.bytes.max(bytes),
            descriptors: previous.descriptors.max(descriptors),
            max_single_vec_records: previous.max_single_vec_records.max(max_single_vec_records),
        });
    });
}

#[cfg(test)]
macro_rules! observe_file_live {
    ($($argument:expr),+ $(,)?) => { observe_file_live($($argument),+) };
}

#[cfg(not(test))]
macro_rules! observe_file_live {
    ($($argument:expr),+ $(,)?) => {{}};
}

#[derive(Clone)]
struct FamilyRun {
    path: PathBuf,
    count: u64,
}

#[cfg(test)]
fn family_run_storage_bytes(run: &FamilyRun) -> usize {
    std::mem::size_of::<FamilyRun>().saturating_add(run.path.capacity())
}

fn write_triple(writer: &mut BufWriter<File>, (a, b, c): Triple) -> Result<(), BuildPipelineError> {
    writer.write_all(&a.to_le_bytes())?;
    writer.write_all(&b.to_le_bytes())?;
    writer.write_all(&c.to_le_bytes())?;
    Ok(())
}

fn create_scratch_writer(path: &std::path::Path) -> Result<BufWriter<File>, BuildPipelineError> {
    Ok(BufWriter::new(
        OpenOptions::new().write(true).create_new(true).open(path)?,
    ))
}

fn write_run(
    temp: &BuildTemp,
    name: &str,
    triples: Vec<Triple>,
) -> Result<FamilyRun, BuildPipelineError> {
    let mut triples = triples;
    radix_sort(&mut triples, &[2, 1, 0])?;
    triples.dedup();
    let count = u64::try_from(triples.len()).map_err(|_| overflow("family run count"))?;
    let path = temp.path(name)?;
    let mut writer = create_scratch_writer(&path)?;
    for triple in triples {
        write_triple(&mut writer, triple)?;
    }
    writer.flush()?;
    Ok(FamilyRun { path, count })
}

#[cfg(test)]
fn write_run_with_live(
    temp: &BuildTemp,
    name: &str,
    mut triples: Vec<Triple>,
    additional_triple_capacities: &[usize],
    additional_io_bytes: usize,
    additional_descriptors: usize,
) -> Result<FamilyRun, BuildPipelineError> {
    let mut radix_scratch = Vec::new();
    radix_sort_with_scratch(&mut triples, &[2, 1, 0], &mut radix_scratch)?;
    triples.dedup();
    let count = u64::try_from(triples.len()).map_err(|_| overflow("family run count"))?;
    let path = temp.path(name)?;
    let mut writer = create_scratch_writer(&path)?;
    observe_file_live!(
        &[triples.capacity(), radix_scratch.capacity()],
        additional_triple_capacities,
        &[],
        0,
        writer.capacity().saturating_add(additional_io_bytes),
        1usize.saturating_add(additional_descriptors),
    );
    for triple in triples {
        write_triple(&mut writer, triple)?;
    }
    writer.flush()?;
    Ok(FamilyRun { path, count })
}

struct RunReader {
    reader: BufReader<File>,
    remaining: u64,
    current: Option<Triple>,
}

impl RunReader {
    fn open(run: &FamilyRun) -> Result<Self, BuildPipelineError> {
        let expected = run
            .count
            .checked_mul(12)
            .ok_or_else(|| overflow("family run byte length"))?;
        let actual = std::fs::metadata(&run.path)?.len();
        if actual != expected {
            return Err(BuildPipelineError::InvalidSpool(
                "family run length does not match count",
            ));
        }
        let mut reader = Self {
            reader: BufReader::new(File::open(&run.path)?),
            remaining: run.count,
            current: None,
        };
        reader.advance()?;
        Ok(reader)
    }

    fn advance(&mut self) -> Result<(), BuildPipelineError> {
        if self.remaining == 0 {
            self.current = None;
            return Ok(());
        }
        let mut bytes = [0u8; 12];
        self.reader.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                BuildPipelineError::InvalidSpool("partial family run record")
            } else {
                error.into()
            }
        })?;
        self.remaining -= 1;
        self.current = Some((
            u32::from_le_bytes(
                bytes[0..4]
                    .try_into()
                    .map_err(|_| overflow("family run a"))?,
            ),
            u32::from_le_bytes(
                bytes[4..8]
                    .try_into()
                    .map_err(|_| overflow("family run b"))?,
            ),
            u32::from_le_bytes(
                bytes[8..12]
                    .try_into()
                    .map_err(|_| overflow("family run c"))?,
            ),
        ));
        Ok(())
    }

    fn next(&mut self) -> Result<Option<Triple>, BuildPipelineError> {
        let current = self.current;
        if current.is_some() {
            self.advance()?;
        }
        Ok(current)
    }
}

fn merge_runs(
    temp: &BuildTemp,
    name: &str,
    inputs: &[FamilyRun],
) -> Result<FamilyRun, BuildPipelineError> {
    if inputs.len() > FILE_RUN_FANIN {
        return Err(BuildPipelineError::InvalidSpool("family merge fan-in"));
    }
    let mut readers: Vec<_> = inputs
        .iter()
        .map(RunReader::open)
        .collect::<Result<_, _>>()?;
    let path = temp.path(name)?;
    let mut writer = create_scratch_writer(&path)?;
    #[cfg(test)]
    let io_buffer_bytes = readers.iter().fold(writer.capacity(), |total, reader| {
        total.saturating_add(reader.reader.capacity())
    });
    observe_file_live!(
        &[],
        &[],
        &[],
        readers
            .capacity()
            .saturating_mul(std::mem::size_of::<RunReader>())
            .saturating_add(inputs.iter().fold(0usize, |total, run| {
                total.saturating_add(family_run_storage_bytes(run))
            })),
        io_buffer_bytes,
        readers.len().saturating_add(1),
    );
    let mut last = None;
    let mut count = 0u64;
    loop {
        let selected = readers
            .iter()
            .enumerate()
            .filter_map(|(index, reader)| reader.current.map(|triple| (index, triple)))
            .min_by_key(|(_, triple)| *triple);
        let Some((index, triple)) = selected else {
            break;
        };
        let _ = readers[index].next()?;
        if last != Some(triple) {
            write_triple(&mut writer, triple)?;
            last = Some(triple);
            count = count
                .checked_add(1)
                .ok_or_else(|| overflow("merged family run count"))?;
        }
    }
    writer.flush()?;
    Ok(FamilyRun { path, count })
}

#[derive(Clone, Copy)]
struct ManagedRun {
    id: u64,
    count: u64,
}

const MANIFEST_RECORD_BYTES: u64 = 16;

fn managed_run_name(label: &str, id: u64) -> String {
    format!("family-{label}-run-{id}")
}

fn managed_run_path(
    temp: &BuildTemp,
    label: &str,
    run: ManagedRun,
) -> Result<PathBuf, BuildPipelineError> {
    temp.path(&managed_run_name(label, run.id))
}

struct RunManifestWriter {
    writer: BufWriter<File>,
}

impl RunManifestWriter {
    fn create(path: &std::path::Path) -> Result<Self, BuildPipelineError> {
        Ok(Self {
            writer: create_scratch_writer(path)?,
        })
    }

    fn append(&mut self, run: ManagedRun) -> Result<(), BuildPipelineError> {
        self.writer.write_all(&run.id.to_le_bytes())?;
        self.writer.write_all(&run.count.to_le_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BuildPipelineError> {
        self.writer.flush()?;
        Ok(())
    }
}

struct RunManifestReader {
    reader: BufReader<File>,
    remaining: u64,
}

impl RunManifestReader {
    fn open(path: &std::path::Path) -> Result<Self, BuildPipelineError> {
        let remaining = std::fs::metadata(path)?.len();
        if remaining % MANIFEST_RECORD_BYTES != 0 {
            return Err(BuildPipelineError::InvalidSpool(
                "partial family run manifest",
            ));
        }
        Ok(Self {
            reader: BufReader::new(File::open(path)?),
            remaining,
        })
    }

    fn next(&mut self) -> Result<Option<ManagedRun>, BuildPipelineError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let mut bytes = [0u8; MANIFEST_RECORD_BYTES as usize];
        self.reader.read_exact(&mut bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                BuildPipelineError::InvalidSpool("partial family run manifest")
            } else {
                error.into()
            }
        })?;
        self.remaining -= MANIFEST_RECORD_BYTES;
        Ok(Some(ManagedRun {
            id: u64::from_le_bytes(
                bytes[0..8]
                    .try_into()
                    .map_err(|_| overflow("family run manifest id"))?,
            ),
            count: u64::from_le_bytes(
                bytes[8..16]
                    .try_into()
                    .map_err(|_| overflow("family run manifest count"))?,
            ),
        }))
    }
}

fn next_run_id(sequence: &mut u64) -> Result<u64, BuildPipelineError> {
    let id = *sequence;
    *sequence = sequence
        .checked_add(1)
        .ok_or_else(|| overflow("family run id"))?;
    Ok(id)
}

fn write_managed_run(
    temp: &BuildTemp,
    label: &str,
    sequence: &mut u64,
    triples: Vec<Triple>,
) -> Result<ManagedRun, BuildPipelineError> {
    let id = next_run_id(sequence)?;
    let run = write_run(temp, &managed_run_name(label, id), triples)?;
    Ok(ManagedRun {
        id,
        count: run.count,
    })
}

#[cfg(test)]
fn write_managed_run_with_live(
    temp: &BuildTemp,
    label: &str,
    sequence: &mut u64,
    triples: Vec<Triple>,
    additional_triple_capacities: &[usize],
    additional_io_bytes: usize,
    additional_descriptors: usize,
) -> Result<ManagedRun, BuildPipelineError> {
    let id = next_run_id(sequence)?;
    let run = write_run_with_live(
        temp,
        &managed_run_name(label, id),
        triples,
        additional_triple_capacities,
        additional_io_bytes,
        additional_descriptors,
    )?;
    Ok(ManagedRun {
        id,
        count: run.count,
    })
}

#[cfg(test)]
macro_rules! write_generated_run {
    ($temp:expr, $label:expr, $sequence:expr, $triples:expr, $capacities:expr, $io:expr, $descriptors:expr $(,)?) => {
        write_managed_run_with_live(
            $temp,
            $label,
            $sequence,
            $triples,
            $capacities,
            $io,
            $descriptors,
        )
    };
}

#[cfg(not(test))]
macro_rules! write_generated_run {
    ($temp:expr, $label:expr, $sequence:expr, $triples:expr, $capacities:expr, $io:expr, $descriptors:expr $(,)?) => {
        write_managed_run($temp, $label, $sequence, $triples)
    };
}

fn merge_managed_runs(
    temp: &BuildTemp,
    label: &str,
    sequence: &mut u64,
    inputs: &[ManagedRun],
) -> Result<ManagedRun, BuildPipelineError> {
    let mut resolved = Vec::with_capacity(inputs.len());
    for &input in inputs {
        resolved.push(FamilyRun {
            path: managed_run_path(temp, label, input)?,
            count: input.count,
        });
    }
    let id = next_run_id(sequence)?;
    let merged = merge_runs(temp, &managed_run_name(label, id), &resolved)?;
    for input in resolved {
        std::fs::remove_file(input.path)?;
    }
    Ok(ManagedRun {
        id,
        count: merged.count,
    })
}

fn consolidate_manifest(
    temp: &BuildTemp,
    label: &str,
    mut manifest: PathBuf,
    sequence: &mut u64,
) -> Result<FamilyRun, BuildPipelineError> {
    loop {
        let next_manifest = temp.path(&format!(
            "family-{label}-manifest-{}",
            next_run_id(sequence)?
        ))?;
        let mut reader = RunManifestReader::open(&manifest)?;
        let mut writer = RunManifestWriter::create(&next_manifest)?;
        let mut outputs = 0usize;
        let mut only = None;
        loop {
            let mut batch = Vec::with_capacity(FILE_RUN_FANIN);
            while batch.len() < FILE_RUN_FANIN {
                let Some(run) = reader.next()? else { break };
                batch.push(run);
            }
            if batch.is_empty() {
                break;
            }
            // The manifest reader/writer stay live around the bounded merge.
            // Its run readers use the same actual BufReader capacity as this
            // manifest reader, and its output uses the writer capacity.
            observe_file_live!(
                &[],
                &[],
                &[],
                batch
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ManagedRun>())
                    .saturating_add(batch.len().saturating_mul(std::mem::size_of::<FamilyRun>()))
                    .saturating_add(manifest.capacity())
                    .saturating_add(next_manifest.capacity()),
                reader
                    .reader
                    .capacity()
                    .saturating_add(writer.writer.capacity())
                    .saturating_add(reader.reader.capacity().saturating_mul(batch.len()))
                    .saturating_add(writer.writer.capacity()),
                batch.len().saturating_add(3),
            );
            let merged = merge_managed_runs(temp, label, sequence, &batch)?;
            writer.append(merged)?;
            outputs = outputs
                .checked_add(1)
                .ok_or_else(|| overflow("family merge batch count"))?;
            only = Some(merged);
        }
        writer.flush()?;
        drop(writer);
        drop(reader);
        std::fs::remove_file(&manifest)?;
        if outputs == 0 {
            std::fs::remove_file(&next_manifest)?;
            let empty = write_managed_run(temp, label, sequence, Vec::new())?;
            return Ok(FamilyRun {
                path: managed_run_path(temp, label, empty)?,
                count: empty.count,
            });
        }
        if outputs == 1 {
            std::fs::remove_file(&next_manifest)?;
            let run = only.ok_or(BuildPipelineError::InvalidSpool("missing family run"))?;
            return Ok(FamilyRun {
                path: managed_run_path(temp, label, run)?,
                count: run.count,
            });
        }
        manifest = next_manifest;
    }
}

fn sorted_file_runs(
    spool: &TripleSpool,
    temp: &BuildTemp,
    family: IndexFamily,
    tile_budget: usize,
    sequence: &mut u64,
) -> Result<(FamilyRun, FamilyRun), BuildPipelineError> {
    let cap = file_run_record_cap(tile_budget);
    #[cfg(test)]
    let source_reader_buffer_bytes = spool.file_reader_buffer_capacity()?;
    let mut first_buffer = Vec::with_capacity(cap);
    let mut second_buffer = Vec::with_capacity(cap);
    let first_manifest = temp.path(&format!("family-first-manifest-{}", next_run_id(sequence)?))?;
    let second_manifest = temp.path(&format!(
        "family-second-manifest-{}",
        next_run_id(sequence)?
    ))?;
    let mut first_runs = RunManifestWriter::create(&first_manifest)?;
    let mut second_runs = RunManifestWriter::create(&second_manifest)?;
    spool.for_each_block(cap, &mut |block| {
        for &triple in block {
            first_buffer.push(family.first(triple));
            second_buffer.push(family.second(triple));
            // The file spool allocates its callback Vec at `cap`; both sibling
            // buffers and the two live manifests contribute their actual
            // capacities. A moved buffer's radix scratch is observed inside
            // `write_run` at the same allocation boundary.
            observe_file_live!(
                &[cap, first_buffer.capacity(), second_buffer.capacity()],
                &[],
                &[],
                0,
                first_runs
                    .writer
                    .capacity()
                    .saturating_add(second_runs.writer.capacity())
                    .saturating_add(source_reader_buffer_bytes),
                3,
            );
            if first_buffer.len() == cap {
                first_runs.append(write_generated_run!(
                    temp,
                    "first",
                    sequence,
                    std::mem::take(&mut first_buffer),
                    &[cap, second_buffer.capacity()],
                    first_runs
                        .writer
                        .capacity()
                        .saturating_add(second_runs.writer.capacity())
                        .saturating_add(source_reader_buffer_bytes),
                    3,
                )?)?;
                second_runs.append(write_generated_run!(
                    temp,
                    "second",
                    sequence,
                    std::mem::take(&mut second_buffer),
                    &[cap, first_buffer.capacity()],
                    first_runs
                        .writer
                        .capacity()
                        .saturating_add(second_runs.writer.capacity())
                        .saturating_add(source_reader_buffer_bytes),
                    3,
                )?)?;
                first_buffer = Vec::with_capacity(cap);
                second_buffer = Vec::with_capacity(cap);
            }
        }
        Ok(())
    })?;
    if !first_buffer.is_empty() {
        first_runs.append(write_generated_run!(
            temp,
            "first",
            sequence,
            first_buffer,
            &[cap, second_buffer.capacity()],
            first_runs
                .writer
                .capacity()
                .saturating_add(second_runs.writer.capacity())
                .saturating_add(source_reader_buffer_bytes),
            3,
        )?)?;
        second_runs.append(write_generated_run!(
            temp,
            "second",
            sequence,
            second_buffer,
            &[cap],
            first_runs
                .writer
                .capacity()
                .saturating_add(second_runs.writer.capacity())
                .saturating_add(source_reader_buffer_bytes),
            3,
        )?)?;
    }
    first_runs.flush()?;
    second_runs.flush()?;
    drop(first_runs);
    drop(second_runs);
    Ok((
        consolidate_manifest(temp, "first", first_manifest, sequence)?,
        consolidate_manifest(temp, "second", second_manifest, sequence)?,
    ))
}

#[derive(Clone)]
struct SegmentSizer {
    a: u32,
    min_b: u32,
    max_b: u32,
    min_c: u32,
    max_c: u32,
    count: u64,
    body: usize,
    num_b: u64,
    current_b: Option<u32>,
    num_c: u64,
    previous_c: u32,
}

impl SegmentSizer {
    fn new(a: u32) -> Self {
        Self {
            a,
            min_b: u32::MAX,
            max_b: 0,
            min_c: u32::MAX,
            max_c: 0,
            count: 0,
            body: uvarint_len(a as u64),
            num_b: 0,
            current_b: None,
            num_c: 0,
            previous_c: 0,
        }
    }

    fn push(&mut self, (a, b, c): Triple) -> Result<(), BuildPipelineError> {
        if a != self.a {
            return Err(BuildPipelineError::InvalidSpool("mixed family segment"));
        }
        if self.current_b != Some(b) {
            if self.current_b.is_some() {
                self.body = self
                    .body
                    .checked_add(uvarint_len(self.num_c))
                    .ok_or_else(|| overflow("family segment c count"))?;
                let previous_b = self.current_b.unwrap_or(0);
                self.body = self
                    .body
                    .checked_add(uvarint_len((b - previous_b) as u64))
                    .ok_or_else(|| overflow("family segment b delta"))?;
            } else {
                self.body = self
                    .body
                    .checked_add(uvarint_len(b as u64))
                    .ok_or_else(|| overflow("family segment first b"))?;
            }
            self.current_b = Some(b);
            self.num_b = self
                .num_b
                .checked_add(1)
                .ok_or_else(|| overflow("family segment b count"))?;
            self.num_c = 0;
            self.previous_c = 0;
        }
        self.body = self
            .body
            .checked_add(uvarint_len((c - self.previous_c) as u64))
            .ok_or_else(|| overflow("family segment c delta"))?;
        self.previous_c = c;
        self.num_c = self
            .num_c
            .checked_add(1)
            .ok_or_else(|| overflow("family segment c count"))?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| overflow("family segment count"))?;
        self.min_b = self.min_b.min(b);
        self.max_b = self.max_b.max(b);
        self.min_c = self.min_c.min(c);
        self.max_c = self.max_c.max(c);
        Ok(())
    }

    fn encoded_size(&self) -> Result<usize, BuildPipelineError> {
        let final_c = uvarint_len(self.num_c);
        let trailing = uvarint_len(self.num_b);
        [
            uvarint_len(self.a as u64),
            uvarint_len(self.a as u64),
            uvarint_len(self.min_b as u64),
            uvarint_len(self.max_b as u64),
            uvarint_len(self.min_c as u64),
            uvarint_len(self.max_c as u64),
            uvarint_len(self.count),
            uvarint_len(1),
            self.body,
            final_c,
            trailing,
        ]
        .into_iter()
        .try_fold(0usize, |sum, part| sum.checked_add(part))
        .ok_or_else(|| overflow("family segment size"))
    }

    fn group_summary(&self) -> Result<GroupSummary, BuildPipelineError> {
        if self.current_b.is_none() {
            return Err(BuildPipelineError::InvalidSpool("empty family group"));
        }
        let body_without_a = self
            .body
            .checked_sub(uvarint_len(self.a as u64))
            .and_then(|size| size.checked_add(uvarint_len(self.num_c)))
            .and_then(|size| size.checked_add(uvarint_len(self.num_b)))
            .ok_or_else(|| overflow("family group summary"))?;
        Ok(GroupSummary {
            a: self.a,
            min_b: self.min_b,
            max_b: self.max_b,
            min_c: self.min_c,
            max_c: self.max_c,
            count: self.count,
            body_without_a,
        })
    }
}

fn next_file_group(
    reader: &mut RunReader,
    pending: &mut Option<Triple>,
    temp: &BuildTemp,
    label: &str,
    sequence: &mut u64,
) -> Result<Option<(u32, FamilyRun)>, BuildPipelineError> {
    let first = match pending.take() {
        Some(triple) => Some(triple),
        None => reader.next()?,
    };
    let first = match first {
        Some(triple) => triple,
        None => return Ok(None),
    };
    let a = first.0;
    let name = format!("family-{label}-group-{}", *sequence);
    *sequence = sequence
        .checked_add(1)
        .ok_or_else(|| overflow("family group name"))?;
    let path = temp.path(&name)?;
    let mut writer = create_scratch_writer(&path)?;
    let mut count = 0u64;
    write_triple(&mut writer, first)?;
    count = count
        .checked_add(1)
        .ok_or_else(|| overflow("family group count"))?;
    loop {
        let next = reader.next()?;
        let Some(triple) = next else { break };
        if triple.0 != a {
            *pending = Some(triple);
            break;
        }
        write_triple(&mut writer, triple)?;
        count = count
            .checked_add(1)
            .ok_or_else(|| overflow("family group count"))?;
    }
    writer.flush()?;
    Ok(Some((a, FamilyRun { path, count })))
}

fn group_summary_from_run(run: &FamilyRun) -> Result<GroupSummary, BuildPipelineError> {
    let mut reader = RunReader::open(run)?;
    let first = reader
        .next()?
        .ok_or(BuildPipelineError::InvalidSpool("empty family group"))?;
    let mut summary = SegmentSizer::new(first.0);
    summary.push(first)?;
    while let Some(triple) = reader.next()? {
        summary.push(triple)?;
    }
    summary.group_summary()
}

fn extend_from_run(values: &mut Vec<Triple>, run: &FamilyRun) -> Result<(), BuildPipelineError> {
    let count = usize::try_from(run.count).map_err(|_| overflow("family group count"))?;
    values
        .try_reserve(count)
        .map_err(|_| overflow("family group buffer"))?;
    let mut reader = RunReader::open(run)?;
    while let Some(triple) = reader.next()? {
        values.push(triple);
    }
    Ok(())
}

#[cfg(test)]
fn extend_from_run_with_live_resources(
    values: &mut Vec<Triple>,
    sibling_values: &Vec<Triple>,
    run: &FamilyRun,
    outer_reader_bytes: usize,
) -> Result<(), BuildPipelineError> {
    let count = usize::try_from(run.count).map_err(|_| overflow("family group count"))?;
    values
        .try_reserve(count)
        .map_err(|_| overflow("family group buffer"))?;
    let mut reader = RunReader::open(run)?;
    while let Some(triple) = reader.next()? {
        values.push(triple);
    }
    observe_file_live!(
        &[values.capacity(), sibling_values.capacity()],
        &[],
        &[],
        0,
        reader.reader.capacity().saturating_add(outer_reader_bytes),
        3,
    );
    Ok(())
}

#[cfg(test)]
macro_rules! extend_from_run {
    ($values:expr, $sibling:expr, $run:expr, $outer_bytes:expr $(,)?) => {
        extend_from_run_with_live_resources($values, $sibling, $run, $outer_bytes)
    };
}

#[cfg(not(test))]
macro_rules! extend_from_run {
    ($values:expr, $sibling:expr, $run:expr, $outer_bytes:expr $(,)?) => {
        extend_from_run($values, $run)
    };
}

fn emit_file_continuations_core(
    first_run: &FamilyRun,
    second_run: &FamilyRun,
    budget: usize,
    tiles: &mut Vec<PairedTile>,
    _outer_reader_bytes: usize,
    _outer_descriptors: usize,
) -> Result<(), BuildPipelineError> {
    if first_run.count != second_run.count {
        return Err(BuildPipelineError::InvalidSpool(
            "family sibling lengths differ",
        ));
    }
    let recovery_limit = budget
        .checked_add(MAX_APPEND_RECOVERY_BYTES)
        .ok_or_else(|| overflow("family recovery budget"))?;
    let mut first_reader = RunReader::open(first_run)?;
    let mut second_reader = RunReader::open(second_run)?;
    let mut pending = VecDeque::new();
    loop {
        let start = match pending.pop_front() {
            Some(pair) => Some(pair),
            None => match (first_reader.next()?, second_reader.next()?) {
                (None, None) => None,
                (Some(first), Some(second)) => Some((first, second)),
                _ => {
                    return Err(BuildPipelineError::InvalidSpool(
                        "family sibling lengths differ",
                    ))
                }
            },
        };
        let Some((first_start, second_start)) = start else {
            break;
        };
        let mut first_sizer = SegmentSizer::new(first_start.0);
        let mut second_sizer = SegmentSizer::new(second_start.0);
        let mut first_values = vec![first_start];
        let mut second_values = vec![second_start];
        first_sizer.push(first_start)?;
        second_sizer.push(second_start)?;
        let mut last_valid =
            if first_sizer.encoded_size()? <= budget && second_sizer.encoded_size()? <= budget {
                Some(1)
            } else {
                None
            };
        loop {
            let next = match pending.pop_front() {
                Some(pair) => Some(pair),
                None => match (first_reader.next()?, second_reader.next()?) {
                    (None, None) => None,
                    (Some(first), Some(second)) => Some((first, second)),
                    _ => {
                        return Err(BuildPipelineError::InvalidSpool(
                            "family sibling lengths differ",
                        ))
                    }
                },
            };
            let Some((first, second)) = next else { break };
            first_sizer.push(first)?;
            second_sizer.push(second)?;
            first_values.push(first);
            second_values.push(second);
            observe_file_live!(
                &[first_values.capacity(), second_values.capacity()],
                &[],
                &[pending.capacity()],
                0,
                first_reader
                    .reader
                    .capacity()
                    .saturating_add(second_reader.reader.capacity())
                    .saturating_add(_outer_reader_bytes),
                2usize.saturating_add(_outer_descriptors),
            );
            let first_size = first_sizer.encoded_size()?;
            let second_size = second_sizer.encoded_size()?;
            if first_size <= budget && second_size <= budget {
                last_valid = Some(first_values.len());
            }
            if first_size > recovery_limit || second_size > recovery_limit {
                break;
            }
        }
        let cut = last_valid.ok_or(BuildPipelineError::InvalidSpool(
            "family continuation has no common bounded prefix",
        ))?;
        let first_tail = first_values.split_off(cut);
        let second_tail = second_values.split_off(cut);
        observe_file_live!(
            &[
                first_values.capacity(),
                second_values.capacity(),
                first_tail.capacity(),
                second_tail.capacity(),
            ],
            &[],
            &[pending.capacity()],
            0,
            first_reader
                .reader
                .capacity()
                .saturating_add(second_reader.reader.capacity())
                .saturating_add(_outer_reader_bytes),
            2usize.saturating_add(_outer_descriptors),
        );
        pending.extend(first_tail.into_iter().zip(second_tail));
        tiles.push(encode_pair(&first_values, &second_values, budget)?);
    }
    Ok(())
}

fn emit_file_continuations(
    first_run: &FamilyRun,
    second_run: &FamilyRun,
    budget: usize,
    tiles: &mut Vec<PairedTile>,
) -> Result<(), BuildPipelineError> {
    emit_file_continuations_core(first_run, second_run, budget, tiles, 0, 0)
}

#[cfg(test)]
fn emit_file_continuations_with_outer_resources(
    first_run: &FamilyRun,
    second_run: &FamilyRun,
    budget: usize,
    tiles: &mut Vec<PairedTile>,
    outer_reader_bytes: usize,
    outer_descriptors: usize,
) -> Result<(), BuildPipelineError> {
    emit_file_continuations_core(
        first_run,
        second_run,
        budget,
        tiles,
        outer_reader_bytes,
        outer_descriptors,
    )
}

#[cfg(test)]
macro_rules! emit_file_continuations {
    ($first:expr, $second:expr, $budget:expr, $tiles:expr, $outer_bytes:expr, $outer_descriptors:expr $(,)?) => {
        emit_file_continuations_with_outer_resources(
            $first,
            $second,
            $budget,
            $tiles,
            $outer_bytes,
            $outer_descriptors,
        )
    };
}

#[cfg(not(test))]
macro_rules! emit_file_continuations {
    ($first:expr, $second:expr, $budget:expr, $tiles:expr, $outer_bytes:expr, $outer_descriptors:expr $(,)?) => {
        emit_file_continuations($first, $second, $budget, $tiles)
    };
}

fn build_file_family(
    spool: &TripleSpool,
    family: IndexFamily,
    tile_budget: usize,
) -> Result<FamilyIndex, BuildPipelineError> {
    let scratch = spool.family_build_temp()?;
    let mut sequence = 0u64;
    let (first_run, second_run) =
        sorted_file_runs(spool, &scratch, family, tile_budget, &mut sequence)?;
    let mut first_reader = RunReader::open(&first_run)?;
    let mut second_reader = RunReader::open(&second_run)?;
    let mut first_pending = None;
    let mut second_pending = None;
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
    loop {
        let first = next_file_group(
            &mut first_reader,
            &mut first_pending,
            &scratch,
            "first",
            &mut sequence,
        )?;
        let second = next_file_group(
            &mut second_reader,
            &mut second_pending,
            &scratch,
            "second",
            &mut sequence,
        )?;
        match (first, second) {
            (None, None) => break,
            (Some((first_a, first_group)), Some((second_a, second_group)))
                if first_a == second_a =>
            {
                let first_group_summary = group_summary_from_run(&first_group)?;
                let second_group_summary = group_summary_from_run(&second_group)?;
                let next_first = first_summary.with_group(first_group_summary)?;
                let next_second = second_summary.with_group(second_group_summary)?;
                if next_first.encoded_size()? <= tile_budget
                    && next_second.encoded_size()? <= tile_budget
                {
                    extend_from_run!(
                        &mut current_first,
                        &current_second,
                        &first_group,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                    )?;
                    extend_from_run!(
                        &mut current_second,
                        &current_first,
                        &second_group,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                    )?;
                    observe_file_live!(
                        &[current_first.capacity(), current_second.capacity()],
                        &[],
                        &[],
                        0,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                        2,
                    );
                    first_summary = next_first;
                    second_summary = next_second;
                    std::fs::remove_file(&first_group.path)?;
                    std::fs::remove_file(&second_group.path)?;
                    continue;
                }
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
                    extend_from_run!(
                        &mut current_first,
                        &current_second,
                        &first_group,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                    )?;
                    extend_from_run!(
                        &mut current_second,
                        &current_first,
                        &second_group,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                    )?;
                    observe_file_live!(
                        &[current_first.capacity(), current_second.capacity()],
                        &[],
                        &[],
                        0,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                        2,
                    );
                    first_summary = single_first;
                    second_summary = single_second;
                } else {
                    emit_file_continuations!(
                        &first_group,
                        &second_group,
                        tile_budget,
                        &mut tiles,
                        first_reader
                            .reader
                            .capacity()
                            .saturating_add(second_reader.reader.capacity()),
                        2,
                    )?;
                }
                std::fs::remove_file(&first_group.path)?;
                std::fs::remove_file(&second_group.path)?;
            }
            _ => {
                return Err(BuildPipelineError::InvalidSpool(
                    "file family leading groups differ",
                ))
            }
        }
    }
    flush(
        &mut tiles,
        &mut current_first,
        &mut current_second,
        &mut first_summary,
        &mut second_summary,
    )?;
    drop(first_reader);
    drop(second_reader);
    std::fs::remove_file(first_run.path)?;
    std::fs::remove_file(second_run.path)?;
    Ok(FamilyIndex { family, tiles })
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
    if spool.count() != 0 && tile_budget < MIN_FAMILY_TILE_BUDGET {
        return Err(BuildPipelineError::InvalidSpool(
            "family tile budget below the minimum of 49 bytes",
        ));
    }
    if spool.is_file_backed() {
        return build_file_family(spool, family, tile_budget);
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

        for (first_slice, second_slice) in
            synchronous_slices(first_group, second_group, tile_budget)?
        {
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

    use super::{
        build_family, file_run_record_cap, merge_runs, sorted_file_runs, write_run, IndexFamily,
        RunReader, FILE_PEAK_WORKING,
    };

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

    fn family_parent(label: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "rete-family-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&parent).unwrap();
        parent
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
    fn nonempty_budget_below_family_minimum_is_rejected_before_partitioning() {
        let triples = vec![(7, 0, 127), (7, 16_384, 127), (7, u32::MAX - 1, 127)];
        let result = build_family(&TripleSpool::Resident(triples), IndexFamily::Subject, 25);
        assert!(matches!(
            result,
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(
                "family tile budget below the minimum of 49 bytes"
            ))
        ));
    }

    #[test]
    fn suffix_heavy_fixture_below_family_minimum_is_rejected_cleanly() {
        let triples = vec![
            (7, 126, 3),
            (7, 126, 16_383),
            (7, 126, 16_385),
            (7, 16_385, u32::MAX),
        ];
        let result = build_family(&TripleSpool::Resident(triples), IndexFamily::Subject, 30);
        assert!(matches!(
            result,
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(
                "family tile budget below the minimum of 49 bytes"
            ))
        ));
    }

    #[test]
    fn maximum_width_singleton_fits_at_the_family_minimum() {
        let triple = (u32::MAX, u32::MAX, u32::MAX);
        let family = build_family(
            &TripleSpool::Resident(vec![triple]),
            IndexFamily::Subject,
            49,
        )
        .unwrap();
        assert_eq!(images(&family, true)[0].len(), 49);
        assert_eq!(images(&family, false)[0].len(), 49);
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
            assert_eq!(
                decode(&images(&replayed, true)),
                expected(triples.clone(), family, false),
                "{family:?} first",
            );
            assert_eq!(
                decode(&images(&replayed, false)),
                expected(triples.clone(), family, true),
                "{family:?} second",
            );
            assert_eq!(images(&resident, true), images(&replayed, true));
            assert_eq!(images(&resident, false), images(&replayed, false));
        }
        drop(file);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn bounded_merge_keeps_every_record_from_two_runs() {
        let parent = std::env::temp_dir().join(format!("rete-family-merge-{}", std::process::id()));
        let temp = BuildTemp::new(&parent).unwrap();
        let left = write_run(&temp, "left", fixture()[..5].to_vec()).unwrap();
        let right = write_run(&temp, "right", fixture()[5..].to_vec()).unwrap();
        let merged = merge_runs(&temp, "merged", &[left, right]).unwrap();
        let mut reader = RunReader::open(&merged).unwrap();
        let mut actual = Vec::new();
        while let Some(triple) = reader.next().unwrap() {
            actual.push(triple);
        }
        let mut wanted = fixture();
        wanted.sort_unstable();
        wanted.dedup();
        assert_eq!(actual, wanted);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn run_reader_rejects_declared_count_overflow_and_trailing_records() {
        let parent =
            std::env::temp_dir().join(format!("rete-family-run-corrupt-{}", std::process::id()));
        let temp = BuildTemp::new(&parent).unwrap();
        let overflow_path = temp.path("overflow").unwrap();
        std::fs::write(&overflow_path, [0u8; 12]).unwrap();
        assert!(matches!(
            RunReader::open(&super::FamilyRun {
                path: overflow_path,
                count: u64::from(u32::MAX),
            }),
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(_))
                | Err(crate::build_pipeline::BuildPipelineError::Overflow(_))
        ));
        let partial_manifest = temp.path("partial-manifest").unwrap();
        std::fs::write(&partial_manifest, [0u8; 15]).unwrap();
        assert!(matches!(
            super::RunManifestReader::open(&partial_manifest),
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(_))
        ));
        let trailing_path = temp.path("trailing").unwrap();
        std::fs::write(&trailing_path, [0u8; 24]).unwrap();
        assert!(matches!(
            RunReader::open(&super::FamilyRun {
                path: trailing_path,
                count: 1,
            }),
            Err(crate::build_pipeline::BuildPipelineError::InvalidSpool(_))
        ));
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn paired_run_generation_normalizes_both_orders_without_dropping_records() {
        let parent = std::env::temp_dir().join(format!("rete-family-runs-{}", std::process::id()));
        let temp = BuildTemp::new(&parent).unwrap();
        let triples = fixture();
        let spool = TripleSpool::write_file(&temp, "input", &triples).unwrap();
        let mut sequence = 0;
        let (first, second) =
            sorted_file_runs(&spool, &temp, IndexFamily::Subject, 64, &mut sequence).unwrap();
        for (run, second_order) in [(first, false), (second, true)] {
            let mut reader = RunReader::open(&run).unwrap();
            let mut actual = Vec::new();
            while let Some(triple) = reader.next().unwrap() {
                actual.push(triple);
            }
            assert_eq!(
                actual,
                expected(triples.clone(), IndexFamily::Subject, second_order)
            );
        }
        drop(spool);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn file_family_sorts_multiple_bounded_runs_and_segments_a_hot_group() {
        let budget = 256;
        let triples = hot_subject(4_000);
        assert!(file_run_record_cap(budget) < triples.len());
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "rete-family-bounded-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let temp = BuildTemp::new(&parent).unwrap();
        let file = TripleSpool::write_file(&temp, "hot.tri", &triples).unwrap();
        FILE_PEAK_WORKING.with(|peak| peak.set(super::FileWorkingSet::default()));
        let family = build_family(&file, IndexFamily::Subject, budget).unwrap();
        assert!(family.tiles.len() > 1);
        assert_eq!(
            decode(&images(&family, true)),
            expected(triples.clone(), IndexFamily::Subject, false)
        );
        assert_eq!(
            decode(&images(&family, false)),
            expected(triples.clone(), IndexFamily::Subject, true)
        );
        assert!(family
            .tiles
            .iter()
            .all(|tile| tile.first.len() <= budget && tile.second.len() <= budget));
        let peak = FILE_PEAK_WORKING.with(|peak| peak.get());
        assert!(peak.records < triples.len());
        assert!(
            peak.records <= 3 * file_run_record_cap(budget) + 2 * budget + super::FILE_RUN_FANIN
        );
        assert!(
            peak.bytes > peak.records * std::mem::size_of::<Triple>(),
            "live-byte accounting includes allocation and I/O buffer capacity"
        );
        let byte_cap = 3 * file_run_record_cap(budget) * std::mem::size_of::<Triple>()
            + (super::FILE_RUN_FANIN + 3) * 16 * 1024
            + 4 * budget * std::mem::size_of::<Triple>()
            + 4 * 1024;
        assert!(peak.bytes <= byte_cap);
        assert!(peak.descriptors <= super::FILE_RUN_FANIN + 3);
        assert!(
            peak.max_single_vec_records <= file_run_record_cap(budget).max(2 * budget),
            "no source, radix, or continuation Vec retains the full spool"
        );
        drop(file);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn file_tracker_counts_additional_live_triple_vec_capacity() {
        FILE_PEAK_WORKING.with(|peak| peak.set(super::FileWorkingSet::default()));
        super::observe_file_live(&[1], &[73], &[], 0, 0, 0);
        let peak = FILE_PEAK_WORKING.with(|peak| peak.get());
        assert_eq!(peak.max_single_vec_records, 73);
    }

    #[test]
    fn file_family_reuses_the_spool_owner_for_scratch_and_cleanup() {
        let parent =
            std::env::temp_dir().join(format!("rete-family-owned-scratch-{}", std::process::id()));
        let temp = BuildTemp::new(&parent).unwrap();
        let owned = temp.owned_path().to_path_buf();
        let file = TripleSpool::write_file(&temp, "source", &hot_subject(128)).unwrap();
        build_family(&file, IndexFamily::Subject, 64).unwrap();
        let files: Vec<_> = std::fs::read_dir(&owned)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files, vec![owned.join("source")]);
        assert!(files.iter().all(|path| path.starts_with(&owned)));
        drop(file);
        drop(temp);
        assert!(!owned.exists());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn file_family_preserves_a_source_named_like_its_old_manifest() {
        let parent = family_parent("source-namespace");
        let temp = BuildTemp::new(&parent).unwrap();
        let source_name = "family-first-manifest-0";
        let source = TripleSpool::write_file(&temp, source_name, &hot_subject(128)).unwrap();
        let source_path = temp.path(source_name).unwrap();
        let before = std::fs::read(&source_path).unwrap();

        let family = build_family(&source, IndexFamily::Subject, 64).unwrap();

        assert!(!family.tiles.is_empty());
        assert_eq!(std::fs::read(source_path).unwrap(), before);
        drop(source);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn concurrent_file_family_builds_use_separate_scratch_and_leave_only_source() {
        let parent = family_parent("concurrent-scratch");
        let temp = BuildTemp::new(&parent).unwrap();
        let triples = hot_subject(512);
        let source = TripleSpool::write_file(&temp, "source", &triples).unwrap();
        let resident =
            build_family(&TripleSpool::Resident(triples), IndexFamily::Subject, 64).unwrap();
        let (one, two) = std::thread::scope(|scope| {
            let one = scope.spawn(|| build_family(&source, IndexFamily::Subject, 64));
            let two = scope.spawn(|| build_family(&source, IndexFamily::Subject, 64));
            (one.join().unwrap().unwrap(), two.join().unwrap().unwrap())
        });

        assert_eq!(images(&one, true), images(&resident, true));
        assert_eq!(images(&one, false), images(&resident, false));
        assert_eq!(images(&two, true), images(&resident, true));
        assert_eq!(images(&two, false), images(&resident, false));
        let files: Vec<_> = std::fs::read_dir(temp.owned_path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, vec![std::ffi::OsString::from("source")]);
        drop(source);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn repeated_file_family_builds_do_not_overwrite_root_leftovers() {
        let parent = family_parent("leftover-namespace");
        let temp = BuildTemp::new(&parent).unwrap();
        let source = TripleSpool::write_file(&temp, "source", &hot_subject(128)).unwrap();
        let leftover = temp.path("family-first-run-2").unwrap();
        let sentinel = b"preexisting root artifact";
        std::fs::write(&leftover, sentinel).unwrap();

        build_family(&source, IndexFamily::Subject, 64).unwrap();
        build_family(&source, IndexFamily::Subject, 64).unwrap();

        assert_eq!(std::fs::read(leftover).unwrap(), sentinel);
        drop(source);
        drop(temp);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn hot_group_file_and_resident_tiles_are_byte_identical() {
        let triples = hot_subject(256);
        let resident = build_family(
            &TripleSpool::Resident(triples.clone()),
            IndexFamily::Subject,
            64,
        )
        .unwrap();
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "rete-family-parity-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let temp = BuildTemp::new(&parent).unwrap();
        let file = TripleSpool::write_file(&temp, "hot.tri", &triples).unwrap();
        let replayed = build_family(&file, IndexFamily::Subject, 64).unwrap();
        assert_eq!(images(&replayed, true), images(&resident, true));
        assert_eq!(images(&replayed, false), images(&resident, false));
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
