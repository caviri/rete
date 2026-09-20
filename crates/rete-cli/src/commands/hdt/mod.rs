//! `rete export --format hdt` — Header-Dictionary-Triples.
//!
//! HDT is a compact binary RDF serialization that stays **queryable without
//! being decompressed**: a reader memory-maps it and answers triple patterns
//! against the mapped bytes. Measured on the reference implementation, opening a
//! 1.39 GB HDT costs 50.7 MB of RSS and 0.31 s, whatever the file's size. That
//! is the property it has and a compressed text dump does not, and the reason it
//! is worth writing despite everything below.
//!
//! # Why this is not a streaming export
//!
//! rete's dictionary is already "HDT-style" — shared / subject-only /
//! object-only / predicate sections, per-section dense ids, shared numbered
//! first — so the obvious implementation is to copy the sections across and
//! stream the SPO index behind them. That does not work, for one reason that
//! took a spike to find:
//!
//! **rete stores IRIs as N-Triples tokens, `<http://…>`, and HDT stores them
//! bare.** Removing the brackets changes the sort order two different ways (see
//! [`dict`]), so every term moves, the id remap is a *non-monotonic permutation*,
//! and rete's SPO scan — ascending in rete's ids — is no longer ascending in
//! HDT's. BitmapTriples requires ascending order in the *target* id space.
//!
//! So the triples must be re-sorted after remapping, which means holding them.
//! That is the whole memory story, and it is why this format has a ceiling when
//! the text formats do not.
//!
//! # The ceiling, and why it is enforced rather than documented
//!
//! Two independent limits, both checked **before any work starts**, from counts
//! the file header already carries:
//!
//! 1. **Object dictionary ids must stay under 2^32.** `hdt-cpp`'s index builder
//!    truncates them: `unsigned int objectValue = arrayZ->get(i)` at
//!    `libhdt/src/triples/BitmapTriples.cpp:341`, where `get()` returns
//!    `size_t`. It is unfixed at HEAD. A file above that bound would be written
//!    successfully and then silently answer queries wrongly on the reader's
//!    machine, which is the worst failure available — so we refuse to produce
//!    one. HDT numbers objects `1..=|shared|` then `|shared|+1..` for
//!    object-only, so the bound is `|shared| + |object-only|`.
//! 2. **The remap and sort have to fit in memory.** See [`MEMORY_PER_TERM`] and
//!    [`MEMORY_PER_TRIPLE`], whose constants are measured rather than guessed.
//!
//! The effective limit is whichever binds first, and which one that is depends
//! on the graph: a sparse graph with a huge object vocabulary hits the id cap, a
//! dense one hits memory. Both messages name the real limit and the file's own
//! count, and point at `--format trig --compress zstd`, which has no ceiling.
//!
//! # What is deliberately not here
//!
//! * **No `.index.v1-1`.** With the id cap enforced, `hdt-cpp` builds its own
//!   index correctly on the first object-bound query and caches it next to the
//!   file. Writing a second binary format for no gain is not worth it.
//! * **No HDTQ.** HDT is triples-only; the quad extension is niche and poorly
//!   supported. Named graphs are handled by selecting one, not by encoding them.
//! * **No compression.** Wrapping an mmap-queryable format in a codec destroys
//!   the one property it has; `--compress` with `--format hdt` is refused.

pub(crate) mod codec;
pub(crate) mod dict;
pub(crate) mod triples;

use std::collections::HashMap;
use std::rc::Rc;

use codec::{
    control_info, CiType, DICTIONARY_FORMAT, GLOBAL_FORMAT, HEADER_FORMAT, ORDER_SPO,
    TRIPLES_FORMAT,
};
use dict::{write_pfc_section, TermArena, BLOCK_SIZE};
use triples::BitmapTriples;

/// The largest object dictionary id we will emit.
///
/// `hdt-cpp` truncates object ids to `unsigned int` while building its index
/// (`BitmapTriples.cpp:341`), so 2^32 - 1 is the hard wall. We stop well short of
/// it: the margin costs nothing — no real graph sits in the gap — and it leaves
/// room for an off-by-one in anyone else's reader rather than betting the file on
/// our arithmetic being exactly right at the boundary.
pub(crate) const MAX_OBJECT_IDS: u64 = 4_000_000_000;

// Checked when this file compiles rather than when a test runs: the cap has to
// sit strictly below hdt-cpp's 32-bit truncation (BitmapTriples.cpp:341), and
// not so far below that it would refuse graphs anyone actually has.
const _: () = assert!(MAX_OBJECT_IDS < (1u64 << 32) - 1);
const _: () = assert!(MAX_OBJECT_IDS > 3_000_000_000);

/// Peak bytes of memory per distinct term.
///
/// **Measured, not assumed** — see `dev/export-formats/RESULTS.md` for the runs.
/// It covers the term text, its `Rc` allocation header and the allocator's
/// rounding, the hash index entry, the role byte, the sort permutation, the two
/// id maps, and the arena copy the front coder walks.
///
/// The compressed `dictionary_len` from the header was tried first as a proxy
/// for text volume and abandoned: it is ~25x smaller than the interned form on
/// the files measured, and the ratio varies with the data, so it predicted worse
/// than a flat per-term constant.
///
/// The constant sits above the worst observed value (419 bytes/term), not at the
/// average, because the two failure modes are not symmetric: over-estimating
/// costs a refusal the user lifts with `--memory-budget-mb`, under-estimating
/// costs an OOM partway through an export that cannot be resumed.
pub(crate) const MEMORY_PER_TERM: u64 = 512;

/// Bytes of peak memory per triple: the `(u32, u32, u32)` plus sort slack.
pub(crate) const MEMORY_PER_TRIPLE: u64 = 16;

/// What the gate decided, so the caller can report it.
pub(crate) struct Budget {
    pub(crate) estimated_bytes: u64,
    pub(crate) object_ids: u64,
    pub(crate) terms: u64,
    pub(crate) quads: u64,
    pub(crate) dict_bytes: u64,
}

/// Refuse, before reading anything but the header, if this file cannot be
/// written as HDT.
///
/// Both bounds are computed from file-wide counts, which are **upper bounds** on
/// any single graph's — the right direction for a gate, since the cost of being
/// conservative is a refusal the user can override by selecting a smaller graph,
/// and the cost of being optimistic is an hour of work that ends in an OOM or a
/// file that misbehaves elsewhere.
pub(crate) fn check_limits(rete: &rete_core::Rete, limit_bytes: u64) -> anyhow::Result<Budget> {
    let d = rete.dictionary();
    let object_ids = d.shared_count() as u64 + d.object_only_count() as u64;
    if object_ids >= MAX_OBJECT_IDS {
        anyhow::bail!(
            "this file has {object_ids} distinct objects, and HDT cannot represent more than \
             {MAX_OBJECT_IDS}.\n\
             The reference implementation truncates object dictionary ids to 32 bits while \
             building its query index, so a larger file would be written successfully and then \
             answer queries incorrectly.\n\
             hint: `--format trig --compress zstd` is lossless, compact, and has no such limit."
        );
    }

    let terms = rete.header().term_count;
    let quads = rete.header().quad_count;
    let dict_bytes = rete.header().dictionary_len;
    let estimated_bytes =
        terms.saturating_mul(MEMORY_PER_TERM) + quads.saturating_mul(MEMORY_PER_TRIPLE);
    if estimated_bytes > limit_bytes {
        anyhow::bail!(
            "writing this file as HDT needs roughly {} of memory ({terms} terms, {quads} \
             triples), above the {} limit.\n\
             HDT cannot be written in a single streaming pass: rete stores IRIs with angle \
             brackets and HDT stores them bare, which reorders the dictionary, so every id has \
             to be remapped and the triples re-sorted in the new order.\n\
             hint: raise `--memory-budget-mb`, export one graph with `--graph`, or use \
             `--format trig --compress zstd`, which streams and has no ceiling.",
            human_bytes(estimated_bytes),
            human_bytes(limit_bytes),
        );
    }
    Ok(Budget {
        estimated_bytes,
        object_ids,
        terms,
        quads,
        dict_bytes,
    })
}

pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Interned terms, split by the role(s) they appear in.
#[derive(Default)]
struct Interner {
    ids: HashMap<Rc<[u8]>, u32>,
    text: Vec<Rc<[u8]>>,
    /// Bit 0: seen as a subject. Bit 1: seen as an object.
    roles: Vec<u8>,
}

const ROLE_SUBJECT: u8 = 1;
const ROLE_OBJECT: u8 = 2;

impl Interner {
    /// Intern the HDT form of a canonical N-Triples token, recording its role.
    fn intern(&mut self, token: &str, role: u8) -> u32 {
        let owned = dict::hdt_form(token);
        let text: &[u8] = &owned;
        if let Some(&i) = self.ids.get(text) {
            self.roles[i as usize] |= role;
            return i;
        }
        let rc: Rc<[u8]> = Rc::from(text);
        let i = self.text.len() as u32;
        self.ids.insert(Rc::clone(&rc), i);
        self.text.push(rc);
        self.roles.push(role);
        i
    }

    /// The indices in each of HDT's three node sections, each sorted by the byte
    /// order `hdt-cpp` binary-searches with.
    fn sections(&self) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let mut shared = Vec::new();
        let mut subjects = Vec::new();
        let mut objects = Vec::new();
        for (i, &r) in self.roles.iter().enumerate() {
            match r {
                x if x == ROLE_SUBJECT | ROLE_OBJECT => shared.push(i as u32),
                ROLE_SUBJECT => subjects.push(i as u32),
                _ => objects.push(i as u32),
            }
        }
        let by_text = |v: &mut Vec<u32>| {
            v.sort_unstable_by(|&a, &b| self.text[a as usize].cmp(&self.text[b as usize]))
        };
        by_text(&mut shared);
        by_text(&mut subjects);
        by_text(&mut objects);
        (shared, subjects, objects)
    }
}

/// Everything the writer needs, built from one scan of the graph.
pub(crate) struct Built {
    pub(crate) bytes: Vec<u8>,
    pub(crate) triples: u64,
    pub(crate) shared: u64,
    pub(crate) subjects: u64,
    pub(crate) predicates: u64,
    pub(crate) objects: u64,
}

/// Build the whole HDT image in memory.
///
/// One scan: terms are interned as they arrive and each triple is recorded as
/// three interner indices, so nothing has to be resolved twice. The partition,
/// the sort and the remap all happen afterwards on those indices.
pub(crate) fn build(
    rete: &mut rete_core::Rete,
    graph: Option<&str>,
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
    iris: &mut Option<rete_core::iri::IriReport>,
    source: &str,
) -> Built {
    let mut nodes = Interner::default();
    let mut preds: HashMap<Rc<[u8]>, u32> = HashMap::new();
    let mut pred_text: Vec<Rc<[u8]>> = Vec::new();
    let mut raw: Vec<(u32, u32, u32)> = Vec::new();

    rete.dump_filtered_each(graph, s, p, o, |ts, tp, to| {
        let (ts, tp, to) = crate::commands::export::clean(iris, ts, tp, to);
        let si = nodes.intern(&ts, ROLE_SUBJECT);
        let oi = nodes.intern(&to, ROLE_OBJECT);
        let powned = dict::hdt_form(&tp);
        let ptext: &[u8] = &powned;
        let pi = match preds.get(ptext) {
            Some(&i) => i,
            None => {
                let rc: Rc<[u8]> = Rc::from(ptext);
                let i = pred_text.len() as u32;
                preds.insert(Rc::clone(&rc), i);
                pred_text.push(rc);
                i
            }
        };
        raw.push((si, pi, oi));
    });
    if let Some(g) = graph {
        rete.release_named_graph(g);
    }

    // --- sections, ids, and the remap ---------------------------------------
    let (shared, subj_only, obj_only) = nodes.sections();
    let mut pred_order: Vec<u32> = (0..pred_text.len() as u32).collect();
    pred_order.sort_unstable_by(|&a, &b| pred_text[a as usize].cmp(&pred_text[b as usize]));

    // HDT's MAPPING2: shared terms take ids 1..=|shared| in BOTH roles, then each
    // role's own section continues from |shared|+1.
    let n = nodes.text.len();
    let mut subject_id = vec![0u32; n];
    let mut object_id = vec![0u32; n];
    for (rank, &idx) in shared.iter().enumerate() {
        let id = rank as u32 + 1;
        subject_id[idx as usize] = id;
        object_id[idx as usize] = id;
    }
    let base = shared.len() as u32;
    for (rank, &idx) in subj_only.iter().enumerate() {
        subject_id[idx as usize] = base + rank as u32 + 1;
    }
    for (rank, &idx) in obj_only.iter().enumerate() {
        object_id[idx as usize] = base + rank as u32 + 1;
    }
    let mut predicate_id = vec![0u32; pred_text.len()];
    for (rank, &idx) in pred_order.iter().enumerate() {
        predicate_id[idx as usize] = rank as u32 + 1;
    }

    // --- triples in HDT id space --------------------------------------------
    for t in raw.iter_mut() {
        *t = (
            subject_id[t.0 as usize],
            predicate_id[t.1 as usize],
            object_id[t.2 as usize],
        );
    }
    raw.sort_unstable();
    raw.dedup();
    let bt = BitmapTriples::build(&raw);
    let triple_count = bt.triple_count();

    // --- the dictionary sections --------------------------------------------
    // Rebuild arenas in HDT order so the front coder walks contiguous memory.
    let mut arena = TermArena::with_capacity(n, 0);
    let section = |idx: &[u32], arena: &mut TermArena| -> Vec<u32> {
        idx.iter()
            .map(|&i| arena.push_bytes(&nodes.text[i as usize]) as u32)
            .collect()
    };
    let shared_ord = section(&shared, &mut arena);
    let subjects_ord = section(&subj_only, &mut arena);
    let objects_ord = section(&obj_only, &mut arena);
    let predicates_ord: Vec<u32> = pred_order
        .iter()
        .map(|&i| arena.push_bytes(&pred_text[i as usize]) as u32)
        .collect();
    let size_strings = arena.text_len();

    // --- assemble ------------------------------------------------------------
    let mut out = Vec::new();
    control_info(&mut out, CiType::Global, GLOBAL_FORMAT, &[]);

    let header = header_ntriples(
        source,
        triple_count,
        shared.len() as u64,
        subj_only.len() as u64,
        pred_order.len() as u64,
        obj_only.len() as u64,
        size_strings,
    );
    control_info(
        &mut out,
        CiType::Header,
        HEADER_FORMAT,
        &[("length", header.len().to_string())],
    );
    out.extend_from_slice(header.as_bytes());

    control_info(
        &mut out,
        CiType::Dictionary,
        DICTIONARY_FORMAT,
        // `hdt-cpp` hard-codes MAPPING2 on load and ignores sizeStrings, but the
        // Java implementation reads `mapping`, so both are emitted.
        &[
            ("mapping", "1".to_string()),
            ("sizeStrings", size_strings.to_string()),
        ],
    );
    // FourSectionDictionary::save order: shared, subjects, PREDICATES, objects.
    write_pfc_section(&mut out, &arena, &shared_ord);
    write_pfc_section(&mut out, &arena, &subjects_ord);
    write_pfc_section(&mut out, &arena, &predicates_ord);
    write_pfc_section(&mut out, &arena, &objects_ord);

    control_info(
        &mut out,
        CiType::Triples,
        TRIPLES_FORMAT,
        // `order` is all `save()` writes; the triple count is recovered from
        // arrayZ's length.
        &[("order", ORDER_SPO.to_string())],
    );
    bt.write(&mut out);

    Built {
        bytes: out,
        triples: triple_count,
        shared: shared.len() as u64,
        subjects: subj_only.len() as u64,
        predicates: pred_order.len() as u64,
        objects: obj_only.len() as u64,
    }
}

/// The header payload: plain N-Triples, `S P O .\n` per line.
///
/// `hdt-cpp` parses it and then consults none of it — `FourSectionDictionary`
/// has the property reads commented out and hard-codes MAPPING2 — so this is for
/// humans and for other implementations. It is worth writing well for exactly
/// that reason.
#[allow(clippy::too_many_arguments)]
fn header_ntriples(
    source: &str,
    triples: u64,
    shared: u64,
    subjects: u64,
    predicates: u64,
    objects: u64,
    size_strings: u64,
) -> String {
    const HDT: &str = "http://purl.org/HDT/hdt#";
    const VOID: &str = "http://rdfs.org/ns/void#";
    let base = format!("<{source}>");
    let mut h = String::new();
    let mut t = |s: &str, p: &str, o: &str| {
        h.push_str(s);
        h.push(' ');
        h.push_str(p);
        h.push(' ');
        h.push_str(o);
        h.push_str(" .\n");
    };
    t(
        &base,
        "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
        &format!("<{HDT}Dataset>"),
    );
    t(
        &base,
        "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
        &format!("<{VOID}Dataset>"),
    );
    t(
        &base,
        &format!("<{VOID}triples>"),
        &format!("\"{triples}\""),
    );
    t(
        &base,
        &format!("<{VOID}properties>"),
        &format!("\"{predicates}\""),
    );
    t(
        &base,
        &format!("<{VOID}distinctSubjects>"),
        &format!("\"{}\"", shared + subjects),
    );
    t(
        &base,
        &format!("<{VOID}distinctObjects>"),
        &format!("\"{}\"", shared + objects),
    );
    t(&base, &format!("<{HDT}formatInformation>"), "_:format");
    t("_:format", &format!("<{HDT}dictionary>"), "_:dictionary");
    t("_:format", &format!("<{HDT}triples>"), "_:triples");
    t(
        "_:dictionary",
        "<http://purl.org/dc/terms/format>",
        &format!("<{HDT}dictionaryFour>"),
    );
    t(
        "_:dictionary",
        &format!("<{HDT}dictionarynumSharedSubjectObject>"),
        &format!("\"{shared}\""),
    );
    t(
        "_:dictionary",
        &format!("<{HDT}dictionarymapping>"),
        "\"1\"",
    );
    t(
        "_:dictionary",
        &format!("<{HDT}dictionarysizeStrings>"),
        &format!("\"{size_strings}\""),
    );
    t(
        "_:dictionary",
        &format!("<{HDT}dictionaryblockSize>"),
        &format!("\"{BLOCK_SIZE}\""),
    );
    t(
        "_:triples",
        "<http://purl.org/dc/terms/format>",
        &format!("<{HDT}triplesBitmap>"),
    );
    t(
        "_:triples",
        &format!("<{HDT}triplesnumTriples>"),
        &format!("\"{triples}\""),
    );
    t("_:triples", &format!("<{HDT}triplesOrder>"), "\"SPO\"");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_reads_like_a_size() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(6_000_000_000), "5.6 GiB");
    }

    #[test]
    fn the_header_is_well_formed_ntriples() {
        let h = header_ntriples("file://x.rete", 5, 1, 2, 3, 4, 100);
        for line in h.lines() {
            assert!(line.ends_with(" ."), "not a statement: {line}");
            assert!(line.split(' ').count() >= 4, "too few terms: {line}");
        }
        assert!(h.contains("\"SPO\""));
        assert!(h.contains("dictionarynumSharedSubjectObject> \"1\""));
        // distinctSubjects is shared + subject-only, which is how HDT reports it.
        assert!(h.contains("distinctSubjects> \"3\""), "{h}");
        assert!(h.contains("distinctObjects> \"5\""), "{h}");
    }

    #[test]
    fn interning_records_both_roles_of_a_shared_term() {
        let mut i = Interner::default();
        let a = i.intern("<http://ex/a>", ROLE_SUBJECT);
        let b = i.intern("<http://ex/b>", ROLE_OBJECT);
        let a2 = i.intern("<http://ex/a>", ROLE_OBJECT);
        assert_eq!(a, a2, "the same term interns once");
        assert_eq!(i.roles[a as usize], ROLE_SUBJECT | ROLE_OBJECT);
        assert_eq!(i.roles[b as usize], ROLE_OBJECT);

        let (shared, subjects, objects) = i.sections();
        assert_eq!(shared.len(), 1);
        assert_eq!(subjects.len(), 0);
        assert_eq!(objects.len(), 1);
    }

    #[test]
    fn sections_are_sorted_by_the_stripped_form() {
        let mut i = Interner::default();
        i.intern("<http://ex/b>", ROLE_SUBJECT);
        i.intern("<http://ex/a>", ROLE_SUBJECT);
        i.intern("_:z", ROLE_SUBJECT);
        let (_, subjects, _) = i.sections();
        let texts: Vec<&[u8]> = subjects.iter().map(|&x| &*i.text[x as usize]).collect();
        // '_' (0x5F) sorts before 'h' (0x68) once the brackets are gone.
        assert_eq!(
            texts,
            vec![b"_:z".as_slice(), b"http://ex/a", b"http://ex/b"]
        );
    }
}
