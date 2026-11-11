//! Derived IRI families (format `0x08`, SPEC.md section 5.3).
//!
//! Some files hold millions of IRIs that are a function of terms already in the
//! file: the OpenCitations Index names a citation `…/ci/<citing>-<cited>`, and
//! its two OMIDs are the objects of the citation's `hasCitingEntity` and
//! `hasCitedEntity` triples. Stored as text they are 99.7 % of that file's
//! dictionary. A **family** says how such an IRI is built:
//!
//! - a **template**: literal pieces around `n` holes;
//! - `n` **inputs**, one per hole: a predicate, and an IRI prefix. The text of
//!   hole `k` is what follows the prefix in the object of the subject's triple
//!   with that predicate.
//!
//! The file keeps every id, rank, group count and routing key of the
//! subject-only section, and drops the **bodies** of the chunks whose every
//! term the writer checked, byte for byte, against the template. A chunk with
//! a term that does not derive stays stored whole (the exceptions). The reader
//! rebuilds a term from the index and the object dictionary: [`DerivedTerms`].
//!
//! Only the subject-only section holds derived chunks, and the inputs are
//! objects, which live in the shared and object-only sections, so decoding a
//! derived term never needs another derived term.

use std::sync::{Arc, OnceLock};

use crate::dictionary::Dictionary;
use crate::index::{GraphIndex, Pattern};

/// The dictionary section that holds derived chunks: subject-only.
pub const SECTION: usize = 1;

/// Most families a file may carry.
pub const MAX_FAMILIES: usize = 16;
/// Most holes in one template.
pub const MAX_INPUTS: usize = 8;

/// A malformed spec or root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveError(pub String);

impl std::fmt::Display for DeriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DeriveError {}

fn err<T>(m: impl Into<String>) -> Result<T, DeriveError> {
    Err(DeriveError(m.into()))
}

/// One hole of a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// The predicate whose object fills the hole, as an IRI (no `<>`).
    pub predicate: String,
    /// The object is an IRI that starts with this; the rest is the hole's text.
    pub prefix: String,
}

/// An IRI family: template, inputs, and the chunks of the subject-only section
/// whose bodies are not stored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Family {
    /// The literal pieces: `inputs.len() + 1` of them. The first starts with
    /// `<` and the last ends with `>` (a term is stored as its token); the ones
    /// between holes are not empty, so that a term can be split again.
    pub literals: Vec<String>,
    /// One per hole.
    pub inputs: Vec<Input>,
    /// Chunks of the subject-only section whose bodies are not stored, as
    /// `(first chunk, count)`, ascending and disjoint. Empty in a spec.
    pub runs: Vec<(usize, usize)>,
}

impl Family {
    /// Parse a template spec: an IRI with holes written `{PREDICATE|PREFIX}`,
    /// for example
    /// `https://w3id.org/oc/index/ci/{http://purl.org/spar/cito/hasCitingEntity|https://w3id.org/oc/meta/br/}-{http://purl.org/spar/cito/hasCitedEntity|https://w3id.org/oc/meta/br/}`.
    pub fn parse_spec(spec: &str) -> Result<Family, DeriveError> {
        let mut literals = Vec::new();
        let mut inputs = Vec::new();
        let mut rest = spec;
        let mut lit = String::from("<");
        loop {
            match rest.find('{') {
                None => {
                    if rest.contains('}') {
                        return err("derive-iris: a `}` without a `{`");
                    }
                    lit.push_str(rest);
                    lit.push('>');
                    literals.push(lit);
                    break;
                }
                Some(at) => {
                    lit.push_str(&rest[..at]);
                    let after = &rest[at + 1..];
                    let end = after
                        .find('}')
                        .ok_or_else(|| DeriveError("derive-iris: a `{` without a `}`".into()))?;
                    let hole = &after[..end];
                    let (predicate, prefix) = hole.split_once('|').ok_or_else(|| {
                        DeriveError(format!(
                            "derive-iris: the hole `{{{hole}}}` is not {{PREDICATE|PREFIX}}"
                        ))
                    })?;
                    if predicate.is_empty() {
                        return err("derive-iris: a hole names no predicate");
                    }
                    inputs.push(Input {
                        predicate: predicate.to_string(),
                        prefix: prefix.to_string(),
                    });
                    literals.push(std::mem::take(&mut lit));
                    rest = &after[end + 1..];
                }
            }
        }
        let f = Family {
            literals,
            inputs,
            runs: Vec::new(),
        };
        f.check()?;
        Ok(f)
    }

    /// The spec this family was parsed from (without its runs).
    pub fn spec(&self) -> String {
        let mut s = String::new();
        for (k, lit) in self.literals.iter().enumerate() {
            let mut t = lit.as_str();
            if k == 0 {
                t = t.strip_prefix('<').unwrap_or(t);
            }
            if k + 1 == self.literals.len() {
                t = t.strip_suffix('>').unwrap_or(t);
            }
            s.push_str(t);
            if let Some(i) = self.inputs.get(k) {
                s.push('{');
                s.push_str(&i.predicate);
                s.push('|');
                s.push_str(&i.prefix);
                s.push('}');
            }
        }
        s
    }

    /// Is the template well formed?
    pub fn check(&self) -> Result<(), DeriveError> {
        let n = self.inputs.len();
        if n == 0 || n > MAX_INPUTS {
            return err(format!(
                "derive-iris: a template has 1 to {MAX_INPUTS} holes"
            ));
        }
        if self.literals.len() != n + 1 {
            return err("derive-iris: the literals do not fit the holes");
        }
        if !self.literals[0].starts_with('<') || !self.literals[n].ends_with('>') {
            return err("derive-iris: a template is an IRI");
        }
        for (k, l) in self.literals.iter().enumerate() {
            if k > 0 && k < n && l.is_empty() {
                return err("derive-iris: two holes need a literal between them");
            }
        }
        if self.literals[0].len() < 2 {
            return err("derive-iris: the template has no IRI text before its first hole");
        }
        Ok(())
    }

    /// The term for the hole texts `values`.
    pub fn build(&self, values: &[&str]) -> String {
        let mut s = String::new();
        for (k, lit) in self.literals.iter().enumerate() {
            s.push_str(lit);
            if let Some(v) = values.get(k) {
                s.push_str(v);
            }
        }
        s
    }

    /// The hole texts of `term`, if it has the template's shape. A text runs to
    /// the first occurrence of the next literal, so a term whose text holds that
    /// literal does not split the way it was built: the writer checks that
    /// [`Self::build`] of the split gives the term back, and stores the chunks
    /// where it does not.
    pub fn split<'a>(&self, term: &'a str) -> Option<Vec<&'a str>> {
        let n = self.inputs.len();
        let mut rest = term.strip_prefix(self.literals[0].as_str())?;
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let lit = self.literals[k + 1].as_str();
            if k + 1 < n {
                let at = rest.find(lit)?;
                out.push(&rest[..at]);
                rest = &rest[at + lit.len()..];
            } else {
                out.push(rest.strip_suffix(lit)?);
            }
        }
        Some(out)
    }

    /// The object token (`<iri>`) of hole `k` with text `value`.
    pub fn object_token(&self, k: usize, value: &str) -> String {
        format!("<{}{}>", self.inputs[k].prefix, value)
    }

    /// The text of hole `k` in the object token `object`, if it is an IRI under
    /// the input's prefix with something after it.
    pub fn value_of<'a>(&self, k: usize, object: &'a str) -> Option<&'a str> {
        let v = object
            .strip_prefix('<')?
            .strip_suffix('>')?
            .strip_prefix(self.inputs[k].prefix.as_str())?;
        (!v.is_empty()).then_some(v)
    }

    /// The term the hole objects `objects` (tokens) spell, if each is under its
    /// input's prefix.
    pub fn derive(&self, objects: &[&str]) -> Option<String> {
        if objects.len() != self.inputs.len() {
            return None;
        }
        let mut values = Vec::with_capacity(objects.len());
        for (k, o) in objects.iter().enumerate() {
            values.push(self.value_of(k, o)?);
        }
        Some(self.build(&values))
    }

    /// Does the family's run list include chunk `ci`?
    pub fn covers(&self, ci: usize) -> bool {
        let at = self.runs.partition_point(|&(first, _)| first <= ci);
        at > 0 && ci < self.runs[at - 1].0 + self.runs[at - 1].1
    }

    /// Chunks the family has no body for.
    pub fn chunk_count(&self) -> usize {
        self.runs.iter().map(|r| r.1).sum()
    }
}

/// Encode the families (the unit root of tag 7).
pub fn encode_families(fams: &[Family]) -> Vec<u8> {
    use crate::varint::write_uvarint;
    let lp = |o: &mut Vec<u8>, s: &str| {
        write_uvarint(o, s.len() as u64);
        o.extend_from_slice(s.as_bytes());
    };
    let mut o = Vec::new();
    write_uvarint(&mut o, fams.len() as u64);
    for f in fams {
        write_uvarint(&mut o, f.inputs.len() as u64);
        lp(&mut o, &f.literals[0]);
        for (k, i) in f.inputs.iter().enumerate() {
            lp(&mut o, &i.predicate);
            lp(&mut o, &i.prefix);
            lp(&mut o, &f.literals[k + 1]);
        }
        write_uvarint(&mut o, f.runs.len() as u64);
        for &(first, n) in &f.runs {
            write_uvarint(&mut o, first as u64);
            write_uvarint(&mut o, n as u64);
        }
    }
    o
}

/// Decode the families of a root; every count and length is untrusted.
/// `n_chunks` is the chunk count of the subject-only section.
pub fn decode_families(p: &[u8], n_chunks: usize) -> Result<Vec<Family>, DeriveError> {
    use crate::varint::read_uvarint;
    let uv = |pos: &mut usize| -> Result<u64, DeriveError> {
        let (v, n) = read_uvarint(p.get(*pos..).unwrap_or(&[]))
            .ok_or_else(|| DeriveError("derived-IRI root: truncated varint".into()))?;
        *pos += n;
        Ok(v)
    };
    let text = |pos: &mut usize, len: u64| -> Result<String, DeriveError> {
        let end = usize::try_from(len)
            .ok()
            .and_then(|l| pos.checked_add(l))
            .filter(|&e| e <= p.len())
            .ok_or_else(|| DeriveError("derived-IRI root: a string overruns".into()))?;
        let s = std::str::from_utf8(&p[*pos..end])
            .map_err(|_| DeriveError("derived-IRI root: a string is not UTF-8".into()))?
            .to_string();
        *pos = end;
        Ok(s)
    };
    let mut pos = 0usize;
    let nf = uv(&mut pos)?;
    if nf > MAX_FAMILIES as u64 {
        return err("derived-IRI root: too many families");
    }
    let mut out = Vec::new();
    for _ in 0..nf {
        let ni = uv(&mut pos)?;
        if ni == 0 || ni > MAX_INPUTS as u64 {
            return err("derived-IRI root: bad hole count");
        }
        let mut f = Family::default();
        let l = uv(&mut pos)?;
        f.literals.push(text(&mut pos, l)?);
        for _ in 0..ni {
            let l = uv(&mut pos)?;
            let predicate = text(&mut pos, l)?;
            let l = uv(&mut pos)?;
            let prefix = text(&mut pos, l)?;
            let l = uv(&mut pos)?;
            f.inputs.push(Input { predicate, prefix });
            f.literals.push(text(&mut pos, l)?);
        }
        f.check()?;
        let nr = uv(&mut pos)?;
        if nr > p.len() as u64 {
            return err("derived-IRI root: bad run count");
        }
        let mut prev_end = 0usize;
        for _ in 0..nr {
            let first = usize::try_from(uv(&mut pos)?).unwrap_or(usize::MAX);
            let n = usize::try_from(uv(&mut pos)?).unwrap_or(usize::MAX);
            let end = first.checked_add(n);
            if n == 0 || first < prev_end || end.is_none_or(|e| e > n_chunks) {
                return err("derived-IRI root: a run is out of order or past the chunks");
            }
            prev_end = first + n;
            f.runs.push((first, n));
        }
        out.push(f);
    }
    if pos != p.len() {
        return err("derived-IRI root: trailing bytes");
    }
    let all = elided_runs(&out);
    if all.windows(2).any(|w| w[0].0 + w[0].1 > w[1].0) {
        return err("derived-IRI root: two families claim a chunk");
    }
    Ok(out)
}

/// The chunk ranges of all families, sorted.
pub fn elided_runs(fams: &[Family]) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = fams.iter().flat_map(|f| f.runs.iter().copied()).collect();
    all.sort_unstable();
    all
}

/// The candidate subjects a lookup looks at, at most, for one term.
const MAX_CANDIDATES: usize = 1 << 20;

/// Rebuilds derived terms for a [`Dictionary`]: reads the inputs of an id from
/// the index, decodes them in the dictionary and fills the template.
pub struct DerivedTerms {
    families: Vec<Family>,
    index: OnceLock<Arc<GraphIndex>>,
    /// Per family, the predicate id of each input (`None` when the file has no
    /// such predicate, so nothing derives).
    preds: OnceLock<Vec<Vec<Option<u32>>>>,
    /// The last term rebuilt: a subject is decoded again for each row it is in.
    last: std::sync::Mutex<Option<(u32, String)>>,
}

impl DerivedTerms {
    /// A resolver for `families`.
    pub fn new(families: Vec<Family>) -> Self {
        DerivedTerms {
            families,
            index: OnceLock::new(),
            preds: OnceLock::new(),
            last: std::sync::Mutex::new(None),
        }
    }

    /// The families.
    pub fn families(&self) -> &[Family] {
        &self.families
    }

    /// Give the resolver the index its ids are read from (once).
    pub fn wire(&self, index: Arc<GraphIndex>) {
        let _ = self.index.set(index);
    }

    fn preds(&self, dict: &Dictionary) -> &[Vec<Option<u32>>] {
        self.preds.get_or_init(|| {
            self.families
                .iter()
                .map(|f| {
                    f.inputs
                        .iter()
                        .map(|i| dict.predicate_id(&format!("<{}>", i.predicate)))
                        .collect()
                })
                .collect()
        })
    }

    /// The family that owns subject-only chunk `ci`.
    fn family_of_chunk(&self, ci: usize) -> Option<usize> {
        self.families.iter().position(|f| f.covers(ci))
    }

    /// The object ids of subject `s`'s inputs in family `fi`; `None` unless each
    /// input has exactly one object.
    fn inputs_of(&self, dict: &Dictionary, fi: usize, s: u32) -> Option<Vec<u32>> {
        let index = self.index.get()?;
        let mut out = Vec::with_capacity(self.families[fi].inputs.len());
        // One lookup of the subject's rows (SPO, the tile the prefetch faulted).
        let rows = index.match_pattern((Some(s), None, None));
        for p in self.preds(dict)[fi].iter() {
            let p = (*p)?;
            let mut it = rows.iter().filter(|r| r.1 == p);
            match (it.next(), it.next()) {
                (Some(&(_, _, o)), None) => out.push(o),
                _ => return None,
            }
        }
        Some(out)
    }

    /// The term of subject-only id `local`, rebuilt.
    pub fn term(&self, dict: &Dictionary, local: u32) -> Option<String> {
        if let Ok(g) = self.last.lock() {
            if let Some((l, t)) = g.as_ref() {
                if *l == local {
                    return Some(t.clone());
                }
            }
        }
        let t = self.term_uncached(dict, local)?;
        if let Ok(mut g) = self.last.lock() {
            *g = Some((local, t.clone()));
        }
        Some(t)
    }

    fn term_uncached(&self, dict: &Dictionary, local: u32) -> Option<String> {
        let ci = dict.subject_only_chunk(local)?;
        let fi = self.family_of_chunk(ci)?;
        let s = dict.shared_count().checked_add(local)?;
        let objects = self.inputs_of(dict, fi, s)?;
        let texts: Vec<String> = objects
            .iter()
            .map(|&o| dict.object_term(o))
            .collect::<Option<_>>()?;
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        self.families[fi].derive(&refs)
    }

    /// Fill `out[slot]` for each `(local id, slot)` of `jobs` that is derived.
    pub fn terms(&self, dict: &Dictionary, jobs: &[(u32, usize)], out: &mut [Option<String>]) {
        let mut jobs = jobs.to_vec();
        jobs.sort_unstable();
        let mut ids: Vec<u32> = jobs.iter().map(|j| j.0).collect();
        ids.dedup();
        self.prefetch(dict, &ids);
        // A subject repeats in every row it is in: rebuild each id once.
        let mut last: Option<(u32, Option<String>)> = None;
        for &(local, slot) in &jobs {
            if out[slot].is_some() {
                continue;
            }
            let t = match &last {
                Some((l, t)) if *l == local => t.clone(),
                _ => {
                    let t = self.term(dict, local);
                    last = Some((local, t.clone()));
                    t
                }
            };
            out[slot] = t;
        }
    }

    /// Warm what decoding these subject-only ids reads, in as few batched reads
    /// as the reader can make: the index tiles of their inputs, then the
    /// dictionary chunks of the objects those name. A no-op on a resident file.
    pub fn prefetch(&self, dict: &Dictionary, locals: &[u32]) {
        let Some(index) = self.index.get() else {
            return;
        };
        if !index.is_remote() {
            return;
        }
        let mut pats: Vec<Pattern> = Vec::new();
        let mut wanted: Vec<u32> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for &l in locals {
            let Some(ci) = dict.subject_only_chunk(l) else {
                continue;
            };
            let Some(fi) = self.family_of_chunk(ci) else {
                continue;
            };
            let Some(s) = dict.shared_count().checked_add(l) else {
                continue;
            };
            if !seen.insert(s) {
                continue;
            }
            pats.push((Some(s), None, None));
            wanted.extend(self.preds(dict)[fi].iter().flatten());
        }
        wanted.sort_unstable();
        wanted.dedup();
        let mut at = 0;
        while at < pats.len() {
            at += index.prefetch_probe_tiles(&pats[at..]).max(1);
        }
        let mut nodes = Vec::new();
        for pat in &pats {
            for (_, p, o) in index.match_pattern(*pat) {
                if wanted.contains(&p) {
                    nodes.push(dict.object_node(o));
                }
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
        dict.prefetch_terms(&nodes, &[]);
    }

    /// The object terms a lookup of `term` reads, if it is shaped like a member
    /// of a family: what a batched lookup prefetch adds to its terms.
    pub fn lookup_terms(&self, term: &str) -> Vec<String> {
        let mut out = Vec::new();
        for f in &self.families {
            if let Some(vals) = f.split(term) {
                for (k, v) in vals.iter().enumerate() {
                    out.push(f.object_token(k, v));
                }
            }
        }
        out
    }

    /// The section-local id of `term`, which routes to an elided chunk whose ids
    /// are `first..=last`: the subject in that range whose inputs are the ones
    /// `term` spells. `None` when there is none, which is when the term is not
    /// in the file.
    pub fn lookup(&self, dict: &Dictionary, term: &str, first: u32, last: u32) -> Option<u32> {
        let index = self.index.get()?;
        let ci = dict.subject_only_chunk(first)?;
        let fi = self.family_of_chunk(ci)?;
        let fam = &self.families[fi];
        let vals = fam.split(term)?;
        if fam.build(&vals) != term {
            return None;
        }
        let preds = self.preds(dict)[fi].clone();
        let mut objs = Vec::with_capacity(vals.len());
        for (k, v) in vals.iter().enumerate() {
            objs.push(dict.object_id_exact(&fam.object_token(k, v))?);
        }
        let shared = dict.shared_count();
        let (lo, hi) = (shared.checked_add(first)?, shared.checked_add(last)?);
        // The subjects of the first input within the chunk; the others check
        // each one (their rows sit next to it in the index).
        let mut cands: Vec<u32> = index
            .match_pattern((None, Some(preds[0]?), Some(objs[0])))
            .into_iter()
            .map(|t| t.0)
            .filter(|s| (lo..=hi).contains(s))
            .collect();
        cands.truncate(MAX_CANDIDATES);
        'cand: for s in cands {
            for k in 1..vals.len() {
                if index
                    .match_pattern((Some(s), Some(preds[k]?), Some(objs[k])))
                    .is_empty()
                {
                    continue 'cand;
                }
            }
            return Some(s - shared);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CI: &str = "https://w3id.org/oc/index/ci/{http://purl.org/spar/cito/hasCitingEntity|https://w3id.org/oc/meta/br/}-{http://purl.org/spar/cito/hasCitedEntity|https://w3id.org/oc/meta/br/}";

    #[test]
    fn spec_round_trips() {
        let f = Family::parse_spec(CI).unwrap();
        assert_eq!(f.inputs.len(), 2);
        assert_eq!(f.literals, ["<https://w3id.org/oc/index/ci/", "-", ">"]);
        assert_eq!(f.spec(), CI);
        assert_eq!(
            f.build(&["0601785", "06902211389"]),
            "<https://w3id.org/oc/index/ci/0601785-06902211389>"
        );
        assert_eq!(
            f.split("<https://w3id.org/oc/index/ci/0601785-06902211389>"),
            Some(vec!["0601785", "06902211389"])
        );
        assert_eq!(f.split("<https://w3id.org/oc/index/ci/0601785>"), None);
        assert_eq!(f.split("<http://other/0601785-1>"), None);
        assert_eq!(
            f.value_of(0, "<https://w3id.org/oc/meta/br/0601785>"),
            Some("0601785")
        );
        assert_eq!(f.value_of(0, "<https://w3id.org/oc/meta/br/>"), None);
        assert_eq!(f.value_of(0, "<http://x/1>"), None);
        assert_eq!(
            f.derive(&[
                "<https://w3id.org/oc/meta/br/0601785>",
                "<https://w3id.org/oc/meta/br/06902211389>"
            ])
            .as_deref(),
            Some("<https://w3id.org/oc/index/ci/0601785-06902211389>")
        );
    }

    #[test]
    fn specs_that_cannot_split_are_refused() {
        assert!(Family::parse_spec("http://x/{a|b}{c|d}").is_err());
        assert!(Family::parse_spec("http://x/").is_err());
        assert!(Family::parse_spec("http://x/{a}").is_err());
        assert!(Family::parse_spec("{a|b}/x").is_err());
        assert!(Family::parse_spec("http://x/{a|b").is_err());
    }

    #[test]
    fn root_round_trips_and_refuses_bad_runs() {
        let mut f = Family::parse_spec(CI).unwrap();
        f.runs = vec![(2, 3), (7, 1)];
        let bytes = encode_families(std::slice::from_ref(&f));
        assert_eq!(decode_families(&bytes, 10).unwrap(), vec![f.clone()]);
        assert!(decode_families(&bytes, 7).is_err(), "run past the chunks");
        assert!(decode_families(&bytes[..bytes.len() - 1], 10).is_err());
        let mut g = f.clone();
        g.runs = vec![(2, 3), (3, 1)];
        assert!(decode_families(&encode_families(&[g]), 10).is_err());
        assert!(f.covers(2) && f.covers(4) && !f.covers(5) && f.covers(7) && !f.covers(8));
    }
}
