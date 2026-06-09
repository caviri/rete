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
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(dict: &'a Dictionary) -> Self {
        Resolver {
            dict,
            terms: RefCell::new(HashMap::new()),
            canon_pred: RefCell::new(HashMap::new()),
            nums: RefCell::new(HashMap::new()),
        }
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
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(rete: &'a Rete, slots: Slots) -> Self {
        Ctx {
            rete,
            slots,
            resolver: Resolver::new(rete.dictionary()),
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
