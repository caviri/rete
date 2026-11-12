//! Slot-row solution representation for the query engine.
//!
//! A solution row is a fixed-width `Vec<Option<Val>>` indexed by a per-query
//! [`Slots`] map (variable name → slot), replacing the per-row
//! `BTreeMap<String, String>` bindings: one allocation per row, no string-key
//! clones, and dictionary IDs stay integers through the whole algebra. Terms are
//! resolved to strings only at projection (late materialization), memoized by
//! the per-query [`Resolver`] so each distinct term decodes once.
//!
//! [`Val`]s are **canonical**: any value whose term exists in the dictionary is
//! stored as `Val::Id` (node id preferred over predicate tag), so structural
//! equality on `Val` coincides with term-string equality — joins, DISTINCT and
//! MINUS on rows behave exactly like the previous string comparisons.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::dictionary::Dictionary;
use crate::fastre::Fast;
use crate::file::Rete;

/// The engine behind SPARQL `REGEX` / `REPLACE` and SHACL `sh:pattern`. With
/// the `unicode-regex` feature (on by default) it is `regex`, whose `\b`,
/// `\w`, `\d` and `(?i)` are Unicode-aware: `\bsvobodová\b` matches, and
/// `(?i)á` matches "Á". Without it (a wasm build that keeps its size), it is
/// `regex-lite`: the same syntax, ASCII-only classes and case folding.
#[cfg(feature = "unicode-regex")]
pub(crate) use regex::Regex;
#[cfg(not(feature = "unicode-regex"))]
pub(crate) use regex_lite::Regex;

/// A bound solution value: a tagged dictionary id (node `>= 0`, predicate
/// `-(p+1) < 0`) or a computed string not present in the dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Val {
    Id(i64),
    Str(Arc<str>),
}

/// A solution row: one optional value per slot (`None` = unbound).
pub(crate) type Row = Vec<Option<Val>>;

/// The per-query variable → slot index map.
#[derive(Debug, Default)]
pub(crate) struct Slots {
    names: Vec<String>,
    by_name: HashMap<String, usize>,
}

impl Slots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The slot for `name`, if any.
    pub(crate) fn slot(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// The slot for `name`, adding one if missing.
    pub(crate) fn add(&mut self, name: &str) -> usize {
        if let Some(&i) = self.by_name.get(name) {
            return i;
        }
        let i = self.names.len();
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), i);
        i
    }

    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    pub(crate) fn name(&self, slot: usize) -> &str {
        &self.names[slot]
    }

    /// A fresh all-unbound row sized for this slot map.
    pub(crate) fn empty_row(&self) -> Row {
        vec![None; self.names.len()]
    }
}

/// Byte cap of each of the resolver's per-query memos (decoded terms, parsed
/// numbers). The memos exist so a term repeated across rows decodes once; a
/// query that touches each of a hundred million distinct literals once gains
/// nothing from keeping them all, and that retention — not the dictionary
/// cache — was most of a 15 GB peak on a whole-predicate FILTER scan. When a
/// memo reaches the cap it is cleared and refills: answers never change.
pub(crate) const MEMO_CAP_BYTES: usize = 64 << 20;

/// Per-entry bookkeeping charged on top of a memoized string's bytes: the
/// hash-map slot, the `Arc` header and allocator rounding (approximate).
const MEMO_ENTRY_OVERHEAD: usize = 64;

/// Per-query memoized term resolution over the file's dictionary.
pub(crate) struct Resolver<'a> {
    dict: &'a Dictionary,
    /// tagged id → term string (None = id unknown / corrupt file). Bounded:
    /// see [`MEMO_CAP_BYTES`].
    terms: RefCell<HashMap<i64, Option<Arc<str>>>>,
    /// Approximate heap held by `terms` (key + entry + string bytes).
    terms_bytes: std::cell::Cell<usize>,
    /// predicate-tagged id → canonical tagged id (node id when the term is
    /// also a node, else itself).
    canon_pred: RefCell<HashMap<i64, i64>>,
    /// value → parsed number (memoized `as_number`).
    nums: RefCell<HashMap<Val, Option<f64>>>,
    /// Approximate heap held by `nums`.
    nums_bytes: std::cell::Cell<usize>,
    /// Byte cap of each memo ([`MEMO_CAP_BYTES`]; tests lower it).
    memo_cap: usize,
    /// `(flags, pattern)` → compiled matcher. REGEX in a FILTER runs per row;
    /// compiling per row dominates the match, and a metacharacter-free pattern
    /// doesn't need the regex engine at all.
    regexes: RefCell<HashMap<(String, String), Matcher>>,
    /// `(flags, pattern)` → compiled REPLACE regex (`None`: invalid pattern).
    /// REPLACE in a BIND runs per row, exactly like REGEX in a FILTER.
    replacers: RefCell<Vec<Replacer>>,
    /// Computed term string → its canonical value ([`Self::canon_term`]).
    /// A BIND or an aggregate yields the same few strings over and over (a
    /// namespace, a month, a source name), and canonicalizing one is several
    /// dictionary lookups: on a ranged file, chunk reads. Bounded like `terms`.
    canon: RefCell<HashMap<Arc<str>, Val>>,
    /// Approximate heap held by `canon`.
    canon_bytes: std::cell::Cell<usize>,
    /// The dictionary chunk of each section the last term came from: a scan's
    /// ids are mostly neighbours, and the next one skips the shared chunk
    /// cache and its lock.
    hold: RefCell<crate::dictionary::TermHold>,
}

/// A compiled REPLACE: flags, pattern, the engine's regex (`None`: invalid
/// pattern) and the plain-string shape of the pattern, if it is one.
type Replacer = (String, String, Option<Regex>, Option<Fast>);

/// A compiled REGEX matcher: plain (case-folded) substring search for literal
/// patterns, the regex engine for everything else, `Never` for invalid patterns.
enum Matcher {
    Substring(String),
    /// Pattern pre-lowercased; the text is lowercased per match (full Unicode
    /// folding, same outcome as `(?i)` for the literal patterns this serves).
    SubstringCi(String),
    Regex(Regex),
    /// A shape that needs no engine; the engine covers the texts it declines.
    Fast(Fast, Regex),
    Never,
}

impl Matcher {
    /// `flags` are the SPARQL flags (subset i/m/s/x mapped to inline flags).
    fn compile(pattern: &str, flags: &str) -> Matcher {
        // A literal pattern (no regex metacharacters) is a substring test; the
        // `m`/`s`/`x` flags only alter metacharacter behavior, so only `i`
        // matters for it.
        let is_literal = !pattern.chars().any(|c| ".^$*+?()[]{}|\\".contains(c));
        if is_literal {
            return if flags.contains('i') {
                Matcher::SubstringCi(pattern.to_lowercase())
            } else {
                Matcher::Substring(pattern.to_string())
            };
        }
        let on: String = ['i', 'm', 's', 'x']
            .iter()
            .filter(|c| flags.contains(**c))
            .collect();
        let inline = if on.is_empty() {
            String::new()
        } else {
            format!("(?{on})")
        };
        match Regex::new(&format!("{inline}{pattern}")) {
            Ok(re) => match Fast::parse(pattern, flags) {
                Some(fast) => Matcher::Fast(fast, re),
                None => Matcher::Regex(re),
            },
            Err(_) => Matcher::Never,
        }
    }

    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Substring(p) => text.contains(p.as_str()),
            Matcher::SubstringCi(p) => {
                // ASCII text folds without the per-row `to_lowercase`
                // allocation; non-ASCII falls back to full Unicode lowering
                // (matching `(?i)` for these literal patterns).
                if text.is_ascii() && p.is_ascii() {
                    ascii_ci_contains(text.as_bytes(), p.as_bytes())
                } else {
                    text.to_lowercase().contains(p.as_str())
                }
            }
            Matcher::Regex(re) => re.is_match(text),
            Matcher::Fast(f, re) => f.is_match(text).unwrap_or_else(|| re.is_match(text)),
            Matcher::Never => false,
        }
    }
}

/// Case-insensitive substring search over ASCII bytes (`needle` already
/// lowercased), allocation-free.
fn ascii_ci_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Which slice of the query's driving scan this context evaluates, when a
/// grouped query runs as shards on several threads (`sparql::par_agg`).
///
/// The plan nodes are named by address: the workers share one lowered plan.
/// `leaves` are the scans that split their tile range by `part`; `only_first`
/// are sub-plans that cannot be split and so run in shard 0 alone. Every one of
/// them must be reached exactly once per shard ([`Shard::exact`]): a plan that
/// evaluates them some other way (a pushdown that rewrites the node) would
/// otherwise return its rows once per shard.
#[derive(Default)]
pub(crate) struct Shard {
    /// `(index, count)`; `None` is the whole query.
    part: std::cell::Cell<Option<(usize, usize)>>,
    leaves: RefCell<Vec<usize>>,
    only_first: RefCell<Vec<usize>>,
    /// Joins whose right side is built once, by the calling thread, and
    /// probed by every shard ([`Shard::shared`]).
    builds: RefCell<Vec<usize>>,
    /// What a split node built once for all shards (a join's hash table, a
    /// basic graph pattern's prefix), by `(plan node, part of it)`. Rows are
    /// `Send + Sync`, so the workers read one copy.
    shared: RefCell<SharedState>,
    /// Set while the calling thread only measures the split (how long the
    /// driving scan is): a join builds nothing yet.
    measuring: std::cell::Cell<bool>,
    /// The named nodes reached in this shard, one entry per time.
    reached: RefCell<Vec<usize>>,
    /// The longest tile range a leaf split, in tiles.
    tiles: std::cell::Cell<usize>,
}

/// A leaf's slice: `(plan node, shard index, shard count)`.
pub(crate) type ShardPart = (usize, usize, usize);

/// State built once for every shard, by `(plan node, part of it)`.
pub(crate) type SharedState =
    HashMap<(usize, usize), std::sync::Arc<dyn std::any::Any + Send + Sync>>;

#[cfg_attr(not(feature = "parallel"), allow(dead_code))]
impl Shard {
    /// Name the plan nodes this context splits on (addresses of plan nodes),
    /// and hand it what was already built for all shards.
    pub(crate) fn set_plan(
        &self,
        leaves: Vec<usize>,
        only_first: Vec<usize>,
        builds: Vec<usize>,
        shared: SharedState,
    ) {
        *self.leaves.borrow_mut() = leaves;
        *self.only_first.borrow_mut() = only_first;
        *self.builds.borrow_mut() = builds;
        *self.shared.borrow_mut() = shared;
    }

    /// Only measure from now on (`true`), or build what the shards share.
    pub(crate) fn set_measuring(&self, on: bool) {
        self.measuring.set(on);
    }

    /// Is this the measuring pass? A join then leaves its table unbuilt: a
    /// query whose driving scan turns out too short to split must not pay
    /// for a table the single thread would never build.
    pub(crate) fn measuring(&self) -> bool {
        self.measuring.get()
    }

    /// What has been built for all shards so far (cheap: a map of `Arc`s).
    pub(crate) fn shared_state(&self) -> SharedState {
        self.shared.borrow().clone()
    }

    /// Is `node` a join whose right side this query builds once for all
    /// shards? Records the visit.
    #[inline]
    pub(crate) fn builds(&self, node: usize) -> bool {
        if self.part.get().is_none() || !self.builds.borrow().contains(&node) {
            return false;
        }
        self.reached.borrow_mut().push(node);
        true
    }

    /// The value built once for all shards under `(node, sub)`: the one in
    /// hand, or `make`'s (the calling thread's dry run makes them; a worker
    /// finds them).
    pub(crate) fn shared<T: Send + Sync + 'static>(
        &self,
        node: usize,
        sub: usize,
        make: impl FnOnce() -> T,
    ) -> std::sync::Arc<T> {
        let have = self.shared.borrow().get(&(node, sub)).cloned();
        if let Some(t) = have.and_then(|a| a.downcast::<T>().ok()) {
            return t;
        }
        let made = std::sync::Arc::new(make());
        self.shared.borrow_mut().insert((node, sub), made.clone());
        made
    }

    /// Is this context evaluating a shard (or measuring for one)?
    pub(crate) fn active(&self) -> bool {
        self.part.get().is_some()
    }

    /// What was put under `(node, sub)` for all shards, if anything: a cell
    /// the calling thread made before the workers started, which the first
    /// shard to need it fills.
    pub(crate) fn cell<T: Send + Sync + 'static>(
        &self,
        node: usize,
        sub: usize,
    ) -> Option<std::sync::Arc<T>> {
        let have = self.shared.borrow().get(&(node, sub)).cloned()?;
        have.downcast::<T>().ok()
    }

    /// Evaluate the same shard as `outer`: for the context of a sub-select
    /// that only projects, whose pattern holds the driving scan.
    pub(crate) fn adopt(&self, outer: &Shard) {
        self.part.set(outer.part.get());
        self.measuring.set(outer.measuring.get());
        *self.leaves.borrow_mut() = outer.leaves.borrow().clone();
        *self.only_first.borrow_mut() = outer.only_first.borrow().clone();
        *self.builds.borrow_mut() = outer.builds.borrow().clone();
        *self.shared.borrow_mut() = outer.shared.borrow().clone();
        self.reached.borrow_mut().clear();
        self.tiles.set(0);
    }

    /// Hand `outer` what this context reached, measured and built ([`adopt`](Self::adopt)).
    pub(crate) fn give_back(&self, outer: &Shard) {
        outer
            .reached
            .borrow_mut()
            .extend(self.reached.borrow().iter().copied());
        outer.tiles.set(outer.tiles.get().max(self.tiles.get()));
        let mine = self.shared.borrow();
        let mut theirs = outer.shared.borrow_mut();
        for (k, v) in mine.iter() {
            theirs.entry(*k).or_insert_with(|| v.clone());
        }
    }

    /// A leaf that slices rows it holds, not tiles (a pattern's prefix, a
    /// list of graphs): the bounds of its slice of `len` rows.
    pub(crate) fn applied_rows(&self, part: ShardPart, len: usize) -> (usize, usize) {
        self.applied(part, 0, len)
    }

    /// Evaluate shard `part` next (`None`: the whole query); forgets what the
    /// last one reached.
    pub(crate) fn begin(&self, part: Option<(usize, usize)>) {
        self.part.set(part);
        self.reached.borrow_mut().clear();
        self.tiles.set(0);
    }

    /// Back to the whole query, with no plan nodes named.
    pub(crate) fn clear(&self) {
        self.leaves.borrow_mut().clear();
        self.only_first.borrow_mut().clear();
        self.builds.borrow_mut().clear();
        self.shared.borrow_mut().clear();
        self.measuring.set(false);
        self.begin(None);
    }

    /// The slice the scan of plan node `node` must keep, when it is a leaf.
    #[inline]
    pub(crate) fn part_for(&self, node: usize) -> Option<ShardPart> {
        let (i, n) = self.part.get()?;
        self.leaves.borrow().contains(&node).then_some((node, i, n))
    }

    /// A leaf applied its slice to the tile range `[start, end)`: the bounds
    /// of the slice within it.
    pub(crate) fn applied(&self, part: ShardPart, start: usize, end: usize) -> (usize, usize) {
        let (node, i, n) = part;
        self.reached.borrow_mut().push(node);
        let len = end.saturating_sub(start);
        self.tiles.set(self.tiles.get().max(len));
        let n = n.max(1);
        let cut = |k: usize| start + (len as u128 * k as u128 / n as u128) as usize;
        (cut(i), cut(i + 1))
    }

    /// A leaf that turned out not to be sliceable is evaluated whole by shard
    /// 0 and yields nothing in the others: `true` when this is another shard.
    /// Records the visit (the leaf was reached; it split nothing).
    pub(crate) fn whole_in_first(&self, part: ShardPart) -> bool {
        self.reached.borrow_mut().push(part.0);
        part.1 != 0
    }

    /// Is plan node `node` one that only shard 0 evaluates, and is this
    /// another shard? Records the visit.
    #[inline]
    pub(crate) fn skips(&self, node: usize) -> bool {
        let Some((i, _)) = self.part.get() else {
            return false;
        };
        if !self.only_first.borrow().contains(&node) {
            return false;
        }
        self.reached.borrow_mut().push(node);
        i != 0
    }

    /// Was every named node reached exactly once in this shard?
    pub(crate) fn exact(&self) -> bool {
        let mut want: Vec<usize> = self
            .leaves
            .borrow()
            .iter()
            .chain(self.only_first.borrow().iter())
            .chain(self.builds.borrow().iter())
            .copied()
            .collect();
        want.sort_unstable();
        let mut got = self.reached.borrow().clone();
        got.sort_unstable();
        want == got
    }

    pub(crate) fn tiles(&self) -> usize {
        self.tiles.get()
    }
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(dict: &'a Dictionary) -> Self {
        Resolver {
            dict,
            terms: RefCell::new(HashMap::new()),
            terms_bytes: std::cell::Cell::new(0),
            canon_pred: RefCell::new(HashMap::new()),
            nums: RefCell::new(HashMap::new()),
            nums_bytes: std::cell::Cell::new(0),
            memo_cap: MEMO_CAP_BYTES,
            regexes: RefCell::new(HashMap::new()),
            replacers: RefCell::new(Vec::new()),
            canon: RefCell::new(HashMap::new()),
            canon_bytes: std::cell::Cell::new(0),
            hold: RefCell::new(Default::default()),
        }
    }

    /// Lower the byte cap of each memo: a worker of a sharded query gets its
    /// share, so the workers together hold what one context would.
    #[cfg_attr(not(feature = "parallel"), allow(dead_code))]
    pub(crate) fn set_memo_cap(&mut self, cap: usize) {
        self.memo_cap = cap;
    }

    /// Does `text` match the SPARQL REGEX `pattern` under `flags`? The matcher
    /// is compiled once per query (memoized); an invalid pattern yields no
    /// match rather than erroring.
    #[cfg(test)]
    pub(crate) fn regex_match(&self, pattern: &str, flags: &str, text: &str) -> bool {
        self.regex_try(pattern, flags, text).unwrap_or(false)
    }

    /// [`Self::regex_match`] that tells an invalid pattern (`None`) apart from
    /// no match (`Some(false)`), so the caller can report it.
    pub(crate) fn regex_try(&self, pattern: &str, flags: &str, text: &str) -> Option<bool> {
        let mut map = self.regexes.borrow_mut();
        let m = map
            .entry((flags.to_string(), pattern.to_string()))
            .or_insert_with(|| Matcher::compile(pattern, flags));
        match m {
            Matcher::Never => None,
            m => Some(m.is_match(text)),
        }
    }

    /// Why `pattern` does not compile (for the diagnostic); `""` if it does.
    pub(crate) fn regex_error(pattern: &str, flags: &str) -> String {
        let on: String = ['i', 'm', 's', 'x']
            .iter()
            .filter(|c| flags.contains(**c))
            .collect();
        let inline = if on.is_empty() {
            String::new()
        } else {
            format!("(?{on})")
        };
        match Regex::new(&format!("{inline}{pattern}")) {
            Ok(_) => String::new(),
            Err(e) => e
                .to_string()
                .lines()
                .last()
                .unwrap_or("")
                .trim()
                .to_string(),
        }
    }

    /// SPARQL `REPLACE(text, pattern, replacement [, flags])`: every match of
    /// `pattern` (with the i/m/s/x flag subset) is replaced, `$N` referring to
    /// capture groups. Returns `None` for an invalid pattern (a type error).
    /// The regex is compiled once per query (memoized), like REGEX: a BIND
    /// runs it on every row, and compiling cost more than the replace itself.
    pub(crate) fn regex_replace(
        &self,
        pattern: &str,
        flags: &str,
        text: &str,
        replacement: &str,
    ) -> Option<String> {
        let mut memo = self.replacers.borrow_mut();
        // A query has a handful of REPLACE patterns: a scan by reference beats
        // hashing (and allocating) an owned `(flags, pattern)` key per row.
        let at = match memo
            .iter()
            .position(|(f, p, _, _)| f == flags && p == pattern)
        {
            Some(i) => i,
            None => {
                let on: String = ['i', 'm', 's', 'x']
                    .iter()
                    .filter(|c| flags.contains(**c))
                    .collect();
                let inline = if on.is_empty() {
                    String::new()
                } else {
                    format!("(?{on})")
                };
                let re = Regex::new(&format!("{inline}{pattern}")).ok();
                let fast = re.as_ref().and_then(|_| Fast::parse(pattern, flags));
                memo.push((flags.to_string(), pattern.to_string(), re, fast));
                memo.len() - 1
            }
        };
        let re = memo[at].2.as_ref()?;
        if let Some(out) = memo[at]
            .3
            .as_ref()
            .and_then(|f| f.replace(text, replacement))
        {
            return Some(out);
        }
        Some(re.replace_all(text, replacement).into_owned())
    }

    /// Coalesce the dictionary chunk faults for a set of values about to be
    /// resolved (a bounded result page): batch-fault their chunks in a few
    /// range reads instead of one per distinct term. Strings carry their own
    /// text and need no fetch. No-op for a local dictionary.
    pub(crate) fn prefetch<'v>(&self, vals: impl Iterator<Item = &'v Val>) {
        let mut nodes = Vec::new();
        let mut preds = Vec::new();
        for v in vals {
            if let Val::Id(id) = v {
                if *id >= 0 {
                    nodes.push(*id as u32);
                } else {
                    preds.push((-id - 1) as u32);
                }
            }
        }
        self.dict.prefetch_terms(&nodes, &preds);
    }

    /// Canonicalize the predicates `pred_ids` ([`canon_id`](Self::canon_id)) in
    /// one go: their terms' chunks are faulted together, then the chunks that
    /// look those terms up as nodes, so the `canon_id` calls that follow are
    /// memo and cache hits. Each of the two steps is one batched read where a
    /// lookup predicate by predicate was up to three sequential reads for each
    /// new one. Only warms the cache; a no-op on a resident dictionary.
    ///
    /// `decode_nodes` are node ids the caller is about to resolve to strings
    /// (a result page): they ride along with the second step's lookups, so
    /// their leaves and chunks cost no round trip of their own.
    pub(crate) fn prefetch_canon(&self, pred_ids: &[u32], decode_nodes: &[u32]) {
        if !self.dict.faults_chunks() {
            return;
        }
        let mut fresh: Vec<u32> = {
            let memo = self.canon_pred.borrow();
            pred_ids
                .iter()
                .copied()
                .filter(|&p| !memo.contains_key(&(-(p as i64) - 1)))
                .collect()
        };
        fresh.sort_unstable();
        fresh.dedup();
        if fresh.is_empty() {
            return;
        }
        self.dict.prefetch_terms(&[], &fresh);
        let terms: Vec<Arc<str>> = fresh
            .iter()
            .filter_map(|&p| self.term(-(p as i64) - 1))
            .collect();
        self.dict.prefetch_lookups_and_terms(
            terms.iter().map(|t| &**t),
            [],
            false,
            decode_nodes,
            &[],
        );
    }

    /// The term string for a tagged id (memoized).
    pub(crate) fn term(&self, id: i64) -> Option<Arc<str>> {
        if let Some(t) = self.terms.borrow().get(&id) {
            return t.clone();
        }
        let t: Option<Arc<str>> = {
            let mut hold = self.hold.borrow_mut();
            if id >= 0 {
                self.dict
                    .node_term_held(id as u32, &mut hold)
                    .map(|t| Arc::from(crate::terms::canonical_literal(&t).as_ref()))
            } else {
                self.dict
                    .predicate_term_held((-id - 1) as u32, &mut hold)
                    .map(Arc::from)
            }
        };
        let add = MEMO_ENTRY_OVERHEAD + t.as_ref().map_or(0, |t| t.len());
        let mut map = self.terms.borrow_mut();
        // A scan that resolves every literal of a big predicate (a FILTER over
        // all titles) would otherwise keep every term it ever decoded: 15 GB on
        // DataCite. Past the cap the memo starts over — it is only a cache, so
        // dropping it costs re-decodes, never a different answer.
        if self.terms_bytes.get() + add > self.memo_cap {
            map.clear();
            self.terms_bytes.set(0);
        }
        map.insert(id, t.clone());
        self.terms_bytes.set(self.terms_bytes.get() + add);
        t
    }

    /// Where a value sorts for `ORDER BY` without decoding it: `(section,
    /// section-local id)` when the file's dictionary is in the SPARQL order and
    /// the id is in an order-exact group (`Dictionary::order_slot`). Two values
    /// of one section compare as their local ids do. `None` for a computed
    /// string, a typed literal, and any value of a byte-ordered file.
    pub(crate) fn order_slot(&self, v: &Val) -> Option<(u8, u32)> {
        match v {
            Val::Id(id) if *id >= 0 => self.dict.order_slot(*id as u32),
            Val::Id(id) => self.dict.predicate_order_slot((-id - 1) as u32),
            Val::Str(_) => None,
        }
    }

    /// How many terms of dictionary section `section` sort before `term`
    /// ([`Dictionary::rank_in_section`]).
    pub(crate) fn rank_in(&self, section: u8, term: &str) -> Option<u32> {
        self.dict.rank_in_section(section as usize, term)
    }

    /// The term string of a value (memoized for ids, shared for strings).
    pub(crate) fn str_of(&self, v: &Val) -> Option<Arc<str>> {
        match v {
            Val::Id(id) => self.term(*id),
            Val::Str(s) => Some(s.clone()),
        }
    }

    /// The term string of a value as an owned `String`, *without* touching the
    /// memo. For the final projection boundary, where most terms are seen once:
    /// memoizing there pays a hash-insert + `Arc` allocation + an extra copy per
    /// term for no hits.
    pub(crate) fn str_once(&self, v: &Val) -> Option<String> {
        match v {
            // A file built before ingest canonicalized `"x"^^xsd:string` may
            // hold the typed spelling; results always show the plain one.
            Val::Id(id) if *id >= 0 => self
                .dict
                .node_term(*id as u32)
                .map(crate::terms::into_canonical_literal),
            Val::Id(id) => self.dict.predicate_term((-id - 1) as u32),
            Val::Str(s) => Some(s.to_string()),
        }
    }

    /// Canonicalize a tagged id: a predicate whose term is also a node becomes
    /// the node id, so equality across subject/object/predicate roles matches
    /// term-string equality (memoized; the predicate space is small).
    pub(crate) fn canon_id(&self, id: i64) -> i64 {
        if id >= 0 {
            return id;
        }
        if let Some(&c) = self.canon_pred.borrow().get(&id) {
            return c;
        }
        let c = self
            .term(id)
            .and_then(|t| self.dict.node_of_term(&t))
            .map(|n| n as i64)
            .unwrap_or(id);
        self.canon_pred.borrow_mut().insert(id, c);
        c
    }

    /// Canonicalize a term string into a value: a dictionary node id when the
    /// term is a node, a predicate tag when it is only a predicate, else the
    /// string itself. Guarantees `Val::Str` is never term-equal to any `Val::Id`.
    pub(crate) fn canon_term(&self, term: &str) -> Val {
        if let Some(v) = self.canon.borrow().get(term) {
            return v.clone();
        }
        let key: Arc<str> = Arc::from(term);
        let v = if let Some(n) = self.dict.node_of_term(term) {
            Val::Id(n as i64)
        } else if let Some(p) = self.dict.predicate_id(term) {
            Val::Id(-(p as i64) - 1)
        } else {
            Val::Str(key.clone())
        };
        let add = MEMO_ENTRY_OVERHEAD + term.len();
        let mut map = self.canon.borrow_mut();
        if self.canon_bytes.get() + add > self.memo_cap {
            map.clear();
            self.canon_bytes.set(0);
        }
        map.insert(key, v.clone());
        self.canon_bytes.set(self.canon_bytes.get() + add);
        v
    }

    /// The numeric value of `v` (literal lexical form or bare number), memoized.
    pub(crate) fn num(&self, v: &Val) -> Option<f64> {
        if let Some(&n) = self.nums.borrow().get(v) {
            return n;
        }
        let n = self
            .str_of(v)
            .as_deref()
            .and_then(crate::sparql::term_number);
        let add = MEMO_ENTRY_OVERHEAD
            + match v {
                Val::Str(s) => s.len(),
                Val::Id(_) => 0,
            };
        let mut map = self.nums.borrow_mut();
        if self.nums_bytes.get() + add > self.memo_cap {
            map.clear();
            self.nums_bytes.set(0);
        }
        map.insert(v.clone(), n);
        self.nums_bytes.set(self.nums_bytes.get() + add);
        n
    }
}

/// Shared evaluation context: the file, the query's slot map, and the
/// memoizing resolver.
pub(crate) struct Ctx<'a> {
    pub(crate) rete: &'a Rete,
    pub(crate) slots: Slots,
    pub(crate) resolver: Resolver<'a>,
    /// An upper bound on how many solutions the consumer will pull, when the
    /// query shape guarantees one (pure LIMIT/OFFSET, or 1 for ASK). Purely a
    /// *strategy* hint — joins switch to index probing under a small bound —
    /// never a correctness input: any plan must yield the same multiset.
    /// A `Cell` so fully-consuming sub-evaluations (EXISTS) can suspend it.
    pub(crate) limit_hint: std::cell::Cell<Option<usize>>,
    /// The dual of `limit_hint`: the consumer PROVABLY pulls the plan
    /// iterator to exhaustion. Set only where full consumption is a fact of
    /// the pipeline, not a guess from the pattern: a SELECT with no LIMIT, or
    /// whose LIMIT sits behind a blocking stage (aggregation, ORDER BY), and
    /// fully-collecting sub-evaluations (EXISTS). Deliberately NOT set for
    /// ASK, `plan_exists`, or DISTINCT … LIMIT — those stop early even though
    /// `limit_hint` is `None`. Purely a *fetch strategy* hint: an unrestricted
    /// `GRAPH ?g` walk over a lazy remote file switches from incremental to
    /// bulk section reads — results are identical either way.
    pub(crate) exhaustive: std::cell::Cell<bool>,
    /// Which string predicates (`diag::MISS_*` bits) already reported a
    /// no-match on valid string arguments to the diagnostics sink, so the
    /// case-sensitivity probe costs one bit test per row after the first.
    pub(crate) miss_mask: std::cell::Cell<u8>,
    /// The top-level SELECT's final decode, when its rows are decoded as they
    /// come out of the plan (no aggregate, ORDER BY or HAVING between): how
    /// many rows at most, and which slots are projected. Only a *fetch
    /// strategy* hint: a batch that canonicalizes a variable predicate also
    /// faults the terms of the rows the projection will decode, in the same
    /// round trips. Set only when the plan is one triple pattern (so every
    /// scanned row reaches the projection), and the rows prefetched are capped
    /// across the whole scan (`bgp::CanonAhead`), not per run. Never an input
    /// to an answer.
    pub(crate) decode_ahead: std::cell::RefCell<Option<(usize, Vec<bool>)>>,
    /// The slice of the driving scan this context evaluates ([`Shard`]); the
    /// whole query unless a grouped query is running as shards.
    pub(crate) shard: Shard,
}

impl<'a> Ctx<'a> {
    /// How many rows the final projection decodes at most (0: none known).
    pub(crate) fn decode_rows(&self) -> usize {
        self.decode_ahead.borrow().as_ref().map_or(0, |d| d.0)
    }

    /// Does the final projection decode slot `i`?
    pub(crate) fn decodes_slot(&self, i: usize) -> bool {
        self.decode_ahead
            .borrow()
            .as_ref()
            .is_some_and(|d| d.1.get(i).copied().unwrap_or(false))
    }

    pub(crate) fn new(rete: &'a Rete, slots: Slots) -> Self {
        Ctx {
            rete,
            slots,
            resolver: Resolver::new(rete.dictionary()),
            limit_hint: std::cell::Cell::new(None),
            exhaustive: std::cell::Cell::new(false),
            miss_mask: std::cell::Cell::new(0),
            decode_ahead: std::cell::RefCell::new(None),
            shard: Shard::default(),
        }
    }
}

/// Two rows are compatible when they agree on every slot bound in both.
pub(crate) fn compatible_rows(a: &Row, b: &Row) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| match (x, y) {
        (Some(v), Some(w)) => v == w,
        _ => true,
    })
}

/// Merge two rows if compatible (shared slots agree), else `None`.
pub(crate) fn merge_rows(a: &Row, b: &Row) -> Option<Row> {
    let mut out = a.clone();
    for (slot, v) in b.iter().enumerate() {
        if let Some(w) = v {
            match &out[slot] {
                Some(existing) if existing != w => return None,
                Some(_) => {}
                None => out[slot] = Some(w.clone()),
            }
        }
    }
    Some(out)
}

/// Which slots are bound in at least one row.
pub(crate) fn bound_mask(rows: &[Row], nslots: usize) -> Vec<bool> {
    let mut mask = vec![false; nslots];
    for r in rows {
        for (i, v) in r.iter().enumerate() {
            if v.is_some() {
                mask[i] = true;
            }
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{write_file_v05 as write_file, Rete};
    use crate::index::GraphIndexBuilder;

    fn fixture() -> Rete {
        let triples = [
            ("<s>", "<shared>", "<shared>"),
            (
                "<s>",
                "<pred-only>",
                "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            ),
            ("<other>", "<shared>", "\"not-a-number\""),
        ];
        let mut builder = DictionaryBuilder::new();
        for (s, p, o) in triples {
            builder.observe(s, p, o);
        }
        let dict = builder.build();
        let mut index = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            index.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &index.build(), false, &[], 0);
        Rete::open(&bytes).unwrap()
    }

    #[test]
    fn slots_rows_merge_and_masks_cover_boundaries() {
        let mut slots = Slots::new();
        assert_eq!(slots.add("x"), 0);
        assert_eq!(slots.add("y"), 1);
        assert_eq!(slots.add("x"), 0);
        assert_eq!(slots.slot("missing"), None);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots.name(1), "y");
        assert_eq!(slots.empty_row(), vec![None, None]);

        let one = Val::Str(Arc::from("one"));
        let two = Val::Str(Arc::from("two"));
        let a = vec![Some(one.clone()), None];
        let b = vec![Some(one.clone()), Some(two.clone())];
        let conflict = vec![Some(two.clone()), None];
        assert!(compatible_rows(&a, &b));
        assert!(!compatible_rows(&a, &conflict));
        assert_eq!(merge_rows(&a, &b), Some(b.clone()));
        assert_eq!(merge_rows(&a, &conflict), None);
        assert_eq!(bound_mask(&[a, b], 3), vec![true, true, false]);
        assert_eq!(bound_mask(&[], 0), Vec::<bool>::new());
    }

    #[test]
    fn resolver_memos_are_bounded_and_answers_survive_a_reset() {
        let rete = fixture();
        let dict = rete.dictionary();
        let mut resolver = Resolver::new(dict);
        // Room for about two entries: the third forces a reset.
        resolver.memo_cap = 2 * MEMO_ENTRY_OVERHEAD + 20;
        let ids: Vec<i64> = ["<s>", "<shared>", "<other>"]
            .iter()
            .map(|t| dict.node_of_term(t).unwrap() as i64)
            .collect();
        for &id in &ids {
            assert!(resolver.term(id).is_some());
            assert!(resolver.terms_bytes.get() <= resolver.memo_cap);
        }
        assert!(resolver.terms.borrow().len() < ids.len(), "memo was reset");
        // Every id still resolves to the same term after the reset.
        for &id in &ids {
            assert_eq!(resolver.term(id), dict.node_term(id as u32).map(Arc::from));
        }
        let n = dict
            .node_of_term("\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>")
            .unwrap() as i64;
        for _ in 0..4 {
            assert_eq!(resolver.num(&Val::Id(n)), Some(42.0));
            assert_eq!(
                resolver.num(&Val::Str(Arc::from(
                    "\"7\"^^<http://www.w3.org/2001/XMLSchema#integer>"
                ))),
                Some(7.0)
            );
            assert!(resolver.nums_bytes.get() <= resolver.memo_cap);
        }
    }

    #[test]
    fn matcher_fast_paths_regex_flags_and_invalid_patterns() {
        assert!(Matcher::compile("Needle", "").is_match("a Needle here"));
        assert!(!Matcher::compile("Needle", "").is_match("needle"));
        assert!(Matcher::compile("Needle", "i").is_match("a nEeDlE here"));
        assert!(Matcher::compile("Ä", "i").is_match("ärger"));
        assert!(Matcher::compile("", "i").is_match("anything"));
        assert!(Matcher::compile("^a.+z$", "is").is_match("A\nZ"));
        assert!(!Matcher::compile("[", "").is_match("anything"));
        assert!(ascii_ci_contains(b"ABCdef", b"cde"));
        assert!(!ascii_ci_contains(b"abc", b"abcd"));
    }

    #[test]
    fn resolver_canonicalizes_decodes_and_memoizes_all_value_kinds() {
        let rete = fixture();
        let dict = rete.dictionary();
        let resolver = Resolver::new(dict);
        let shared_node = dict.node_of_term("<shared>").unwrap() as i64;
        let shared_pred = -(dict.predicate_id("<shared>").unwrap() as i64) - 1;
        let pred_only = -(dict.predicate_id("<pred-only>").unwrap() as i64) - 1;
        let number = dict
            .node_of_term("\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>")
            .unwrap() as i64;

        resolver.prefetch(
            [
                Val::Id(shared_node),
                Val::Id(shared_pred),
                Val::Str(Arc::from("owned")),
            ]
            .iter(),
        );
        assert_eq!(&*resolver.term(shared_node).unwrap(), "<shared>");
        assert_eq!(&*resolver.term(shared_node).unwrap(), "<shared>");
        assert_eq!(resolver.term(999_999), None);
        assert_eq!(resolver.term(-999_999), None);
        assert_eq!(
            &*resolver.str_of(&Val::Id(shared_pred)).unwrap(),
            "<shared>"
        );
        assert_eq!(
            &*resolver.str_of(&Val::Str(Arc::from("raw"))).unwrap(),
            "raw"
        );
        assert_eq!(
            resolver.str_once(&Val::Id(shared_node)).as_deref(),
            Some("<shared>")
        );
        assert_eq!(
            resolver.str_once(&Val::Id(pred_only)).as_deref(),
            Some("<pred-only>")
        );
        assert_eq!(
            resolver.str_once(&Val::Str(Arc::from("raw"))).as_deref(),
            Some("raw")
        );

        assert_eq!(resolver.canon_id(shared_node), shared_node);
        assert_eq!(resolver.canon_id(shared_pred), shared_node);
        assert_eq!(resolver.canon_id(shared_pred), shared_node);
        assert_eq!(resolver.canon_id(pred_only), pred_only);
        assert_eq!(resolver.canon_term("<shared>"), Val::Id(shared_node));
        assert_eq!(resolver.canon_term("<pred-only>"), Val::Id(pred_only));
        assert_eq!(
            resolver.canon_term("<unknown>"),
            Val::Str(Arc::from("<unknown>"))
        );

        assert_eq!(resolver.num(&Val::Id(number)), Some(42.0));
        assert_eq!(resolver.num(&Val::Id(number)), Some(42.0));
        let bad = resolver.canon_term("\"not-a-number\"");
        assert_eq!(resolver.num(&bad), None);
        assert_eq!(resolver.num(&bad), None);

        assert!(resolver.regex_match("shared", "i", "SHARED value"));
        assert!(resolver.regex_match("shared", "i", "shared again"));
        assert!(!resolver.regex_match("[", "", "shared"));
        assert_eq!(
            resolver.regex_replace("(a+)", "i", "Aa bb", "<$1>"),
            Some("<Aa> bb".to_string())
        );
        assert_eq!(resolver.regex_replace("[", "", "x", "y"), None);
        // Compiled once for each `(flags, pattern)`, the invalid one included.
        for _ in 0..3 {
            assert_eq!(
                resolver.regex_replace("(a+)", "i", "Aa bb", "<$1>"),
                Some("<Aa> bb".to_string())
            );
            assert_eq!(
                resolver.regex_replace("(a+)", "", "Aa bb", "<$1>"),
                Some("A<a> bb".to_string())
            );
            assert_eq!(resolver.regex_replace("[", "", "x", "y"), None);
        }
        assert_eq!(resolver.replacers.borrow().len(), 3);
    }

    /// A computed string is looked up in the dictionary once, then answered
    /// from the memo, with the same value; the memo is bounded like the
    /// others and a reset changes no answer.
    #[test]
    fn canonical_values_are_memoized_and_bounded() {
        let rete = fixture();
        let dict = rete.dictionary();
        let resolver = Resolver::new(dict);
        let shared = Val::Id(dict.node_of_term("<shared>").unwrap() as i64);
        let pred_only = Val::Id(-(dict.predicate_id("<pred-only>").unwrap() as i64) - 1);
        let cases = [
            ("<shared>", shared),
            ("<pred-only>", pred_only),
            ("\"computed\"", Val::Str(Arc::from("\"computed\""))),
        ];
        for _ in 0..2 {
            for (term, want) in &cases {
                assert_eq!(&resolver.canon_term(term), want);
            }
        }
        assert_eq!(resolver.canon.borrow().len(), cases.len());
        // Room for about one entry: every other lookup starts the memo over.
        let mut resolver = Resolver::new(dict);
        resolver.memo_cap = MEMO_ENTRY_OVERHEAD + 16;
        for _ in 0..3 {
            for (term, want) in &cases {
                assert_eq!(&resolver.canon_term(term), want);
                assert!(resolver.canon_bytes.get() <= resolver.memo_cap);
            }
        }
        assert!(resolver.canon.borrow().len() < cases.len());
    }
}
