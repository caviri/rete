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
use std::rc::Rc;

use crate::dictionary::Dictionary;
use crate::file::Rete;

/// A bound solution value: a tagged dictionary id (node `>= 0`, predicate
/// `-(p+1) < 0`) or a computed string not present in the dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Val {
    Id(i64),
    Str(Rc<str>),
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

/// Per-query memoized term resolution over the file's dictionary.
pub(crate) struct Resolver<'a> {
    dict: &'a Dictionary,
    /// tagged id → term string (None = id unknown / corrupt file).
    terms: RefCell<HashMap<i64, Option<Rc<str>>>>,
    /// predicate-tagged id → canonical tagged id (node id when the term is
    /// also a node, else itself).
    canon_pred: RefCell<HashMap<i64, i64>>,
    /// value → parsed number (memoized `as_number`).
    nums: RefCell<HashMap<Val, Option<f64>>>,
    /// `(flags, pattern)` → compiled matcher. REGEX in a FILTER runs per row;
    /// compiling per row dominates the match, and a metacharacter-free pattern
    /// doesn't need the regex engine at all.
    regexes: RefCell<HashMap<(String, String), Matcher>>,
}

/// A compiled REGEX matcher: plain (case-folded) substring search for literal
/// patterns, the regex engine for everything else, `Never` for invalid patterns.
enum Matcher {
    Substring(String),
    /// Pattern pre-lowercased; the text is lowercased per match (full Unicode
    /// folding, same outcome as `(?i)` for the literal patterns this serves).
    SubstringCi(String),
    Regex(regex_lite::Regex),
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
        match regex_lite::Regex::new(&format!("{inline}{pattern}")) {
            Ok(re) => Matcher::Regex(re),
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

impl<'a> Resolver<'a> {
    pub(crate) fn new(dict: &'a Dictionary) -> Self {
        Resolver {
            dict,
            terms: RefCell::new(HashMap::new()),
            canon_pred: RefCell::new(HashMap::new()),
            nums: RefCell::new(HashMap::new()),
            regexes: RefCell::new(HashMap::new()),
        }
    }

    /// Does `text` match the SPARQL REGEX `pattern` under `flags`? The matcher
    /// is compiled once per query (memoized); an invalid pattern yields no
    /// match rather than erroring.
    pub(crate) fn regex_match(&self, pattern: &str, flags: &str, text: &str) -> bool {
        let mut map = self.regexes.borrow_mut();
        let m = map
            .entry((flags.to_string(), pattern.to_string()))
            .or_insert_with(|| Matcher::compile(pattern, flags));
        m.is_match(text)
    }

    /// SPARQL `REPLACE(text, pattern, replacement [, flags])`: every match of
    /// `pattern` (with the i/m/s/x flag subset) is replaced, `$N` referring to
    /// capture groups. Returns `None` for an invalid pattern (a type error).
    /// Not memoized — REPLACE always needs the full engine (no substring fast
    /// path) and is rare relative to FILTER REGEX.
    pub(crate) fn regex_replace(
        &self,
        pattern: &str,
        flags: &str,
        text: &str,
        replacement: &str,
    ) -> Option<String> {
        let on: String = ['i', 'm', 's', 'x']
            .iter()
            .filter(|c| flags.contains(**c))
            .collect();
        let inline = if on.is_empty() {
            String::new()
        } else {
            format!("(?{on})")
        };
        let re = regex_lite::Regex::new(&format!("{inline}{pattern}")).ok()?;
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

    /// The term string for a tagged id (memoized).
    pub(crate) fn term(&self, id: i64) -> Option<Rc<str>> {
        if let Some(t) = self.terms.borrow().get(&id) {
            return t.clone();
        }
        let t: Option<Rc<str>> = if id >= 0 {
            self.dict.node_term(id as u32).map(Rc::from)
        } else {
            self.dict.predicate_term((-id - 1) as u32).map(Rc::from)
        };
        self.terms.borrow_mut().insert(id, t.clone());
        t
    }

    /// The term string of a value (memoized for ids, shared for strings).
    pub(crate) fn str_of(&self, v: &Val) -> Option<Rc<str>> {
        match v {
            Val::Id(id) => self.term(*id),
            Val::Str(s) => Some(s.clone()),
        }
    }

    /// The term string of a value as an owned `String`, *without* touching the
    /// memo. For the final projection boundary, where most terms are seen once:
    /// memoizing there pays a hash-insert + `Rc` allocation + an extra copy per
    /// term for no hits.
    pub(crate) fn str_once(&self, v: &Val) -> Option<String> {
        match v {
            Val::Id(id) if *id >= 0 => self.dict.node_term(*id as u32),
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
        if let Some(n) = self.dict.node_of_term(term) {
            Val::Id(n as i64)
        } else if let Some(p) = self.dict.predicate_id(term) {
            Val::Id(-(p as i64) - 1)
        } else {
            Val::Str(Rc::from(term))
        }
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
        self.nums.borrow_mut().insert(v.clone(), n);
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
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(rete: &'a Rete, slots: Slots) -> Self {
        Ctx {
            rete,
            slots,
            resolver: Resolver::new(rete.dictionary()),
            limit_hint: std::cell::Cell::new(None),
            exhaustive: std::cell::Cell::new(false),
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
    use crate::file::{write_file, Rete};
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

        let one = Val::Str(Rc::from("one"));
        let two = Val::Str(Rc::from("two"));
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
                Val::Str(Rc::from("owned")),
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
            &*resolver.str_of(&Val::Str(Rc::from("raw"))).unwrap(),
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
            resolver.str_once(&Val::Str(Rc::from("raw"))).as_deref(),
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
            Val::Str(Rc::from("<unknown>"))
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
    }
}
