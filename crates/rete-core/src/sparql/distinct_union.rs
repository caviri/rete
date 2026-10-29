//! Evaluation of [`Plan::DistinctUnion`]: the distinct bindings of a few
//! variables over the union of single triple patterns.
//!
//! The OWL 2 QL rewriter ([`super::ql`]) answers an entailed atom by a union of
//! asserted patterns: `?x a C` by `?x a D` for every `D ⊑* C`, plus `?x p ?_`
//! for every `p` whose domain is `⊑* C`, plus `?_ q ?x` for every `q` whose
//! range is. A work typed both `fabio:Expression` and `fabio:JournalArticle`
//! matches two of them, yet the entailed triple `?x a fabio:Expression` exists
//! once — so the union must be deduplicated, and cheaply: a class can have
//! hundreds of millions of members.
//!
//! Each source is scanned through a permutation **sorted on the output
//! variables** (`?x a D` through POS/OPS, `?x p ?_` through PSO, `?_ q ?x`
//! through POS), so every source is a stream of ascending keys, and a k-way
//! merge drops the repeats as they meet. Memory is one cursor per source and the
//! stream is lazy: `LIMIT 10` reads the first tile of each source and stops. A
//! source no carried permutation can stream in key order (a domain source on a
//! three-permutation file, which has no PSO) is collected and sorted instead —
//! correct, just not constant-memory.
//!
//! Joined against rows that already bind every output variable, the node is a
//! semi-join: [`matches`] checks each source with the values substituted, a
//! handful of point lookups per row.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::eval::RowIter;
use crate::bgp::{PatternTerm, TriplePattern};
use crate::index::{GraphIndex, Pattern};
use crate::row::{Ctx, Row, Val};
use crate::triples::Triple;

/// Where one output variable sits in a source: the column that keys it and any
/// further columns that must hold the same node (`?x p ?x`).
struct KeyCol {
    col: usize,
    same: Vec<usize>,
}

/// One source lowered to integer space: its index pattern (constants as role
/// ids, every variable unbound) and, per output variable, where it sits.
struct Lowered {
    pattern: Pattern,
    keys: Vec<KeyCol>,
    /// A column holds a variable that is neither an output variable nor
    /// constrained — the stream may repeat a key between other keys' runs
    /// unless the key columns lead the permutation.
    anonymous: usize,
}

/// Lower one source against the dictionary. `None` = a constant is not in the
/// file in the role this pattern needs, so the source matches nothing.
fn lower(ctx: &Ctx, vars: &[String], src: &TriplePattern) -> Option<Lowered> {
    let dict = ctx.rete.dictionary();
    let terms = [&src.s, &src.p, &src.o];
    let mut pattern: [Option<u32>; 3] = [None, None, None];
    let mut keys: Vec<KeyCol> = vars
        .iter()
        .map(|_| KeyCol {
            col: usize::MAX,
            same: Vec::new(),
        })
        .collect();
    let mut anonymous = 0;
    for (col, t) in terms.into_iter().enumerate() {
        match t {
            PatternTerm::Const(c) => {
                pattern[col] = Some(match col {
                    0 => dict.subject_id(c)?,
                    1 => dict.predicate_id(c)?,
                    _ => dict.object_id(c)?,
                });
            }
            PatternTerm::Var(v) => match vars.iter().position(|x| x == v) {
                Some(i) if keys[i].col == usize::MAX => keys[i].col = col,
                Some(i) => keys[i].same.push(col),
                None => anonymous += 1,
            },
        }
    }
    // An output variable the source does not mention would be left unbound by
    // it; the rewriter never builds one, and treating it as a mismatch keeps
    // the "every output variable is bound" contract.
    if keys.iter().any(|k| k.col == usize::MAX) {
        return None;
    }
    Some(Lowered {
        pattern: (pattern[0], pattern[1], pattern[2]),
        keys,
        anonymous,
    })
}

/// The node value of column `col` of a matched triple, as a row value.
fn column_value(ctx: &Ctx, t: Triple, col: usize) -> i64 {
    let dict = ctx.rete.dictionary();
    match col {
        0 => dict.subject_node(t.0) as i64,
        1 => ctx.resolver.canon_id(-(t.1 as i64) - 1),
        _ => dict.object_node(t.2) as i64,
    }
}

/// The key of a matched triple, or `None` when a repeated variable's columns
/// disagree.
fn key_of(ctx: &Ctx, l: &Lowered, t: Triple) -> Option<Vec<i64>> {
    let mut key = Vec::with_capacity(l.keys.len());
    for k in &l.keys {
        let v = column_value(ctx, t, k.col);
        if k.same.iter().any(|&c| column_value(ctx, t, c) != v) {
            return None;
        }
        key.push(v);
    }
    Some(key)
}

type KeyIter<'q> = Box<dyn Iterator<Item = Vec<i64>> + 'q>;

/// One source as a stream of ascending keys (repeats allowed; the merge drops
/// them).
fn key_stream<'q>(ctx: &'q Ctx<'q>, index: &'q GraphIndex, l: Lowered) -> KeyIter<'q> {
    // Sorted on the first key column; a second key is then in order too only
    // when it is the one remaining free column (no anonymous column between).
    let sortable = match l.keys.len() {
        1 => true,
        2 => l.anonymous == 0,
        _ => false,
    };
    if sortable {
        let lead = l.keys[0].col;
        // A predicate-valued key is canonicalized through the node space,
        // which is not monotone in the predicate id: only node columns stream.
        if lead != 1 && l.keys.get(1).is_none_or(|k| k.col != 1) {
            if let Some(it) = index.scan_iter_sorted_on(l.pattern, lead) {
                return Box::new(it.filter_map(move |t| key_of(ctx, &l, t)));
            }
        }
    }
    let mut keys: Vec<Vec<i64>> = index
        .scan_iter(l.pattern)
        .filter_map(|t| key_of(ctx, &l, t))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    Box::new(keys.into_iter())
}

/// A k-way merge of ascending key streams that yields each key once.
struct MergeDistinct<'q> {
    streams: Vec<KeyIter<'q>>,
    heads: BinaryHeap<Reverse<(Vec<i64>, usize)>>,
    last: Option<Vec<i64>>,
}

impl<'q> MergeDistinct<'q> {
    fn new(mut streams: Vec<KeyIter<'q>>) -> Self {
        let mut heads = BinaryHeap::with_capacity(streams.len());
        for (i, s) in streams.iter_mut().enumerate() {
            if let Some(k) = s.next() {
                heads.push(Reverse((k, i)));
            }
        }
        MergeDistinct {
            streams,
            heads,
            last: None,
        }
    }
}

impl Iterator for MergeDistinct<'_> {
    type Item = Vec<i64>;

    fn next(&mut self) -> Option<Vec<i64>> {
        loop {
            let Reverse((key, i)) = self.heads.pop()?;
            if let Some(k) = self.streams[i].next() {
                self.heads.push(Reverse((k, i)));
            }
            if self.last.as_ref() != Some(&key) {
                self.last = Some(key.clone());
                return Some(key);
            }
        }
    }
}

/// Sources that differ only in one constant — `?x a C₁`, `?x a C₂`, … for the
/// subclasses of a class — form a group. From this many on, a group may be
/// read as ONE scan of its predicate, sorted on the key, keeping the triples
/// whose constant is one of the group's.
const GROUP_SCAN_MIN: usize = 8;

/// A source of the form `?k p c` or `c p ?k`: one key variable, a constant
/// predicate and a constant in the other position. Returns
/// `(predicate, key column, constant column, constant id)`.
fn groupable(l: &Lowered) -> Option<(u32, usize, usize, u32)> {
    let [k] = l.keys.as_slice() else {
        return None;
    };
    if !k.same.is_empty() || l.anonymous != 0 {
        return None;
    }
    let p = l.pattern.1?;
    let (other, id) = match k.col {
        0 => (2, l.pattern.2?),
        2 => (0, l.pattern.0?),
        _ => return None,
    };
    Some((p, k.col, other, id))
}

/// The value of column `col` of a role-id triple.
fn component(t: Triple, col: usize) -> u32 {
    match col {
        0 => t.0,
        1 => t.1,
        _ => t.2,
    }
}

/// The bytes of the UNION of the tiles that scanning each of `patterns`
/// (through its cheapest permutation) routes to. Sources that share tiles —
/// sibling classes in one OPS tile — count those tiles once, which a plain sum
/// of per-pattern figures does not.
fn union_tile_bytes(index: &GraphIndex, patterns: impl Iterator<Item = Pattern>) -> u64 {
    let sections = index.tile_sections();
    let mut tiles: std::collections::BTreeSet<(usize, usize)> = std::collections::BTreeSet::new();
    for pattern in patterns {
        let mut best: Option<(u64, usize, usize, usize)> = None;
        for perm in index.perms().iter() {
            let si = perm.section_index();
            let (start, end) = index.tile_span(si, perm.order_pattern(pattern)[0]);
            let bytes: u64 = sections[si][start..end]
                .iter()
                .map(|t| t.encoded_len())
                .sum();
            if best.is_none_or(|(b, ..)| bytes < b) {
                best = Some((bytes, si, start, end));
            }
        }
        if let Some((_, si, start, end)) = best {
            tiles.extend((start..end).map(|i| (si, i)));
        }
    }
    tiles
        .into_iter()
        .map(|(si, i)| sections[si][i].encoded_len())
        .sum()
}

/// The sorted streams for a group of [`groupable`] sources sharing a predicate
/// and a key column: one stream per source, or ONE scan of the predicate
/// keeping the group's constants, whichever reads less.
///
/// Per-source cursors all start before the first row, so even `LIMIT 10` reads
/// the first tile of every source — for a taxonomy of hundreds of classes, most
/// of the typed extent. One scan reads tiles only as rows
/// are pulled, but a full enumeration reads the predicate's whole extent. So
/// the single scan is taken when it is no larger than the union of the
/// per-source tiles, or, under a small demand bound (`LIMIT`), when it is at
/// most four times larger.
fn group_streams<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    p: u32,
    key_col: usize,
    other_col: usize,
    members: Vec<(Lowered, u32)>,
) -> Vec<KeyIter<'q>> {
    if members.len() >= GROUP_SCAN_MIN {
        let whole: Pattern = (None, Some(p), None);
        let per_source = union_tile_bytes(index, members.iter().map(|(l, _)| l.pattern));
        let one_scan = crate::bgp::routed_scan_bytes(index, whole);
        let slack = if ctx.limit_hint.get().is_some_and(|h| h <= 4096) {
            4
        } else {
            1
        };
        if one_scan <= per_source.saturating_mul(slack) {
            if let Some(it) = index.scan_iter_sorted_on(whole, key_col) {
                let wanted: std::collections::HashSet<u32> =
                    members.iter().map(|&(_, id)| id).collect();
                return vec![Box::new(
                    it.filter(move |&t| wanted.contains(&component(t, other_col)))
                        .map(move |t| vec![column_value(ctx, t, key_col)]),
                )];
            }
        }
    }
    members
        .into_iter()
        .map(|(l, _)| key_stream(ctx, index, l))
        .collect()
}
/// Evaluate a `DistinctUnion` against the active graph `index`.
pub(super) fn iter<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    vars: &'q [String],
    sources: &'q [TriplePattern],
) -> RowIter<'q> {
    let lowered: Vec<Lowered> = sources.iter().filter_map(|s| lower(ctx, vars, s)).collect();
    let Some(slots) = vars
        .iter()
        .map(|v| ctx.slots.slot(v))
        .collect::<Option<Vec<usize>>>()
    else {
        return Box::new(std::iter::empty());
    };
    if vars.is_empty() {
        // An existence test: one empty solution iff some source matches.
        let hit = lowered
            .iter()
            .any(|l| index.scan_iter(l.pattern).next().is_some());
        return if hit {
            Box::new(std::iter::once(ctx.slots.empty_row()))
        } else {
            Box::new(std::iter::empty())
        };
    }
    let mut streams: Vec<KeyIter<'q>> = Vec::new();
    let mut groups: std::collections::BTreeMap<(u32, usize, usize), Vec<(Lowered, u32)>> =
        std::collections::BTreeMap::new();
    for l in lowered {
        match groupable(&l) {
            Some((p, kc, oc, id)) => groups.entry((p, kc, oc)).or_default().push((l, id)),
            None => streams.push(key_stream(ctx, index, l)),
        }
    }
    for ((p, kc, oc), members) in groups {
        streams.extend(group_streams(ctx, index, p, kc, oc, members));
    }
    Box::new(MergeDistinct::new(streams).map(move |key| {
        let mut row = ctx.slots.empty_row();
        for (&slot, v) in slots.iter().zip(key) {
            row[slot] = Some(Val::Id(v));
        }
        row
    }))
}

/// The role id that node value `node` has in column `col`, if any.
fn role_id(ctx: &Ctx, col: usize, node: i64) -> Option<u32> {
    let dict = ctx.rete.dictionary();
    match col {
        0 if node >= 0 => dict.node_as_subject_id(node as u32),
        2 if node >= 0 => dict.node_as_object_id(node as u32),
        // A predicate position: the value is a predicate tag, or a node whose
        // term is also a predicate.
        1 if node < 0 => Some((-node - 1) as u32),
        1 => dict
            .node_term(node as u32)
            .and_then(|t| dict.predicate_id(&t)),
        _ => None,
    }
}

/// Does some source match with every output variable taken from `row`? The
/// semi-join probe: `row` must bind all of `vars` (the caller checks that the
/// other join side certainly does).
///
/// A source whose subject is the bound key and whose object is a constant
/// (`<x> a C₁`, `<x> a C₂`, …) is checked against ONE read of the subject's
/// values for that predicate (`<x> a ?t`, a run in its SPO tile) rather than
/// one lookup per class, each of which may route to a different tile.
pub(super) fn matches(
    ctx: &Ctx,
    index: &GraphIndex,
    vars: &[String],
    sources: &[TriplePattern],
    row: &Row,
) -> bool {
    let mut values: Vec<i64> = Vec::with_capacity(vars.len());
    for v in vars {
        match ctx.slots.slot(v).and_then(|s| row[s].as_ref()) {
            Some(Val::Id(id)) => values.push(*id),
            // A computed term outside the dictionary is in no triple.
            Some(Val::Str(_)) | None => return false,
        }
    }
    // `(subject, predicate)` → the objects it has, read once per row.
    let mut objects: Vec<((u32, u32), Vec<u32>)> = Vec::new();
    sources.iter().any(|src| {
        let Some(l) = lower(ctx, vars, src) else {
            return false;
        };
        let mut pattern = [l.pattern.0, l.pattern.1, l.pattern.2];
        for (k, &node) in l.keys.iter().zip(&values) {
            for col in std::iter::once(k.col).chain(k.same.iter().copied()) {
                match role_id(ctx, col, node) {
                    Some(id) => pattern[col] = Some(id),
                    None => return false,
                }
            }
        }
        let object_is_key = l.keys.iter().any(|k| k.col == 2 || k.same.contains(&2));
        if let ([Some(s), Some(p), Some(o)], false) = (pattern, object_is_key) {
            let at = match objects.iter().position(|(sp, _)| *sp == (s, p)) {
                Some(i) => i,
                None => {
                    let mut os: Vec<u32> = index
                        .scan_iter((Some(s), Some(p), None))
                        .map(|t| t.2)
                        .collect();
                    os.sort_unstable();
                    objects.push(((s, p), os));
                    objects.len() - 1
                }
            };
            return objects[at].1.binary_search(&o).is_ok();
        }
        index
            .scan_iter((pattern[0], pattern[1], pattern[2]))
            .next()
            .is_some()
    })
}

/// The tile bytes a full evaluation would at least scan, summed over the
/// sources (from the tile directory, no read) — the join's "is this side fat"
/// figure.
pub(super) fn scan_bytes(
    ctx: &Ctx,
    index: &GraphIndex,
    vars: &[String],
    sources: &[TriplePattern],
) -> u64 {
    sources
        .iter()
        .filter_map(|s| lower(ctx, vars, s))
        .map(|l| crate::bgp::routed_scan_bytes(index, l.pattern))
        .fold(0u64, u64::saturating_add)
}
