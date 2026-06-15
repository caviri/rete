//! Coherence-tier cost benchmark: how much of a `.rete` each coherence check
//! actually fetches, as the graph grows. Builds a synthetic medical ontology (a
//! fixed T-Box with a planted unsatisfiable class + a few instance-level clashes)
//! at increasing instance counts, then measures, via a `CountingReader` over the
//! file image, the bytes + range requests + time for each tier:
//!
//!   Tier 0 — `read_schema_coherence_ranged` (header + pyramid-meta, dict-free)
//!   Tier 1 — a selective coherence `CONSTRUCT` + `reason` over the slice
//!   Tier 2 — `dump` the whole graph + `reason` (the complete check)
//!
//! Findings (see docs/BENCHMARK.md): Tier 2's reason scales with the graph; Tier 1
//! reads only the rdf:type + T-Box tiles (a small, sub-linear slice); Tier 0 reads
//! only header + pyramid-meta (never the dictionary). Tier 0 is flat for the schema
//! itself, but pyramid-meta also carries the community summary, which grows with the
//! number of distinct entities — so on a large, entity-rich graph Tier 1 is the
//! cheaper remote check until the schema pyramid gets its own addressable section.
//! Run:  cargo run --release --example coherence_bench [n1 n2 …]  (default 1k 10k 100k)

use std::sync::Arc;
use std::time::{Duration, Instant};

use rete_core::{
    build_pyramid_meta, eval_query, read_schema_coherence_ranged, reason, CountingReader,
    DictionaryBuilder, GraphIndexBuilder, QueryOutput, Rete, SliceReader, DEFAULT_TILE_BUDGET,
};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const DISJOINT_WITH: &str = "<http://www.w3.org/2002/07/owl#disjointWith>";
const XSD_INT: &str = "<http://www.w3.org/2001/XMLSchema#integer>";

/// The selective Tier-1 slice (the same shape as `rete-wasm`'s COHERENCE_CONSTRUCT):
/// each branch is a constant predicate, so each routes to one predicate's tiles.
const COHERENCE_CONSTRUCT: &str = "\
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
PREFIX owl: <http://www.w3.org/2002/07/owl#> \
CONSTRUCT { ?x rdf:type ?c . ?sub rdfs:subClassOf ?sup . ?c1 owl:disjointWith ?c2 } \
WHERE { { ?x rdf:type ?c } UNION { ?sub rdfs:subClassOf ?sup } \
        UNION { ?c1 owl:disjointWith ?c2 } }";

type Triple = (String, String, String);

/// A fixed small ontology + `instances` patients. Each patient has 2–3 type
/// triples and ~9 data triples (unique literals → a realistically large
/// dictionary), so the type slice (Tier 1) is a fraction of the file (Tier 2).
fn generate(instances: usize) -> Vec<Triple> {
    let mut t: Vec<Triple> = Vec::with_capacity(instances * 12 + 16);
    let mut push =
        |s: &str, p: &str, o: &str| t.push((s.to_string(), p.to_string(), o.to_string()));

    // --- T-Box (fixed): Relapsed ⊑ Healthy and ⊑ Sick (disjoint) → unsatisfiable.
    push("<http://ex/Patient>", SUBCLASS_OF, "<http://ex/Person>");
    push("<http://ex/Person>", SUBCLASS_OF, "<http://ex/Agent>");
    push("<http://ex/Doctor>", SUBCLASS_OF, "<http://ex/Person>");
    push("<http://ex/Healthy>", DISJOINT_WITH, "<http://ex/Sick>");
    push("<http://ex/Relapsed>", SUBCLASS_OF, "<http://ex/Healthy>");
    push("<http://ex/Relapsed>", SUBCLASS_OF, "<http://ex/Sick>");

    // --- A-Box: scaled instances.
    for i in 0..instances {
        let s = format!("<http://ex/patient/{i}>");
        push(&s, RDF_TYPE, "<http://ex/Patient>");
        // The first 3 are typed into BOTH disjoint classes — an instance-level
        // clash only Tier 1/2 can see; the rest alternate cleanly.
        if i < 3 {
            push(&s, RDF_TYPE, "<http://ex/Healthy>");
            push(&s, RDF_TYPE, "<http://ex/Sick>");
        } else if i % 2 == 0 {
            push(&s, RDF_TYPE, "<http://ex/Healthy>");
        } else {
            push(&s, RDF_TYPE, "<http://ex/Sick>");
        }
        push(
            &s,
            "<http://ex/name>",
            &format!("\"Patient number {i} of the cohort\""),
        );
        push(&s, "<http://ex/mrn>", &format!("\"MRN-{i:09}\""));
        push(
            &s,
            "<http://ex/age>",
            &format!("\"{}\"^^{XSD_INT}", 18 + (i % 70)),
        );
        push(
            &s,
            "<http://ex/visits>",
            &format!("\"{}\"^^{XSD_INT}", i % 20),
        );
        push(
            &s,
            "<http://ex/bloodPressure>",
            &format!("\"{}\"^^{XSD_INT}", 90 + (i % 60)),
        );
        push(
            &s,
            "<http://ex/note>",
            &format!("\"Routine note for record {i}; no action.\""),
        );
        push(
            &s,
            "<http://ex/admittedOn>",
            &format!(
                "\"20{:02}-{:02}-{:02}\"",
                i % 25,
                1 + (i % 12),
                1 + (i % 28)
            ),
        );
        // A bounded relation: refer within a small ward, so the community pyramid
        // stays representative of a typed-instance graph (not a graph-spanning chain
        // that would blow up the superedge summary — an unrelated subsystem).
        if i % 200 != 0 {
            push(
                &s,
                "<http://ex/wardMate>",
                &format!("<http://ex/patient/{}>", i - (i % 200)),
            );
        }
    }
    t
}

fn build(triples: &[Triple]) -> Vec<u8> {
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let encoded: Vec<_> = triples
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).expect("observed"))
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for e in &encoded {
        ib.push(*e);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &encoded, DEFAULT_TILE_BUDGET);
    rete_core::write_dataset(&dict, &ib.build(), &[], false, &meta, levels)
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else if b >= 1 << 10 {
        format!("{:.1} KB", b as f64 / (1u64 << 10) as f64)
    } else {
        format!("{b} B")
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2}ms", d.as_secs_f64() * 1000.0)
}

struct Row {
    bytes: u64,
    reqs: u64,
    time: Duration,
    found: usize,
}

fn tier0(bytes: &'static [u8]) -> Row {
    let reader = CountingReader::new(SliceReader::new(bytes));
    let t = Instant::now();
    let found = read_schema_coherence_ranged(&reader)
        .unwrap()
        .unwrap()
        .len();
    Row {
        bytes: reader.bytes_read(),
        reqs: reader.requests(),
        time: t.elapsed(),
        found,
    }
}

fn tier1(bytes: &'static [u8]) -> Row {
    let reader = Arc::new(CountingReader::new(SliceReader::new(bytes)));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let t = Instant::now();
    let slice = match eval_query(&rete, COHERENCE_CONSTRUCT).unwrap() {
        QueryOutput::Construct(tr) => tr,
        _ => unreachable!(),
    };
    let found = reason(&slice).inconsistencies.len();
    Row {
        bytes: reader.bytes_read(),
        reqs: reader.requests(),
        time: t.elapsed(),
        found,
    }
}

fn tier2(bytes: &'static [u8]) -> Row {
    let reader = Arc::new(CountingReader::new(SliceReader::new(bytes)));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let t = Instant::now();
    let base = rete.dump(None);
    let found = reason(&base).inconsistencies.len();
    Row {
        bytes: reader.bytes_read(),
        reqs: reader.requests(),
        time: t.elapsed(),
        found,
    }
}

fn main() {
    let sizes: Vec<usize> = {
        let args: Vec<usize> = std::env::args()
            .skip(1)
            .filter_map(|a| a.parse().ok())
            .collect();
        if args.is_empty() {
            vec![1_000, 10_000, 100_000]
        } else {
            args
        }
    };

    println!(
        "{:>10}  {:>9}  │  {:^26}  │  {:^26}  │  {:^28}",
        "instances",
        "file",
        "Tier-0 (schema, index-free)",
        "Tier-1 (selective slice)",
        "Tier-2 (full graph)"
    );
    println!(
        "{:>10}  {:>9}  │  {:>9} {:>4}r {:>8}  │  {:>9} {:>4}r {:>8}  │  {:>9} {:>4}r {:>9}",
        "", "", "bytes", "", "time", "bytes", "", "time", "bytes", "", "time"
    );
    println!("{}", "─".repeat(116));

    for n in sizes {
        let triples = generate(n);
        let total = triples.len();
        // Leak the file image so the lazy reader's 'static bound is satisfied
        // (a benchmark process; the OS reclaims it on exit).
        let bytes: &'static [u8] = Box::leak(build(&triples).into_boxed_slice());
        let file = bytes.len() as u64;

        let r0 = tier0(bytes);
        let r1 = tier1(bytes);
        let r2 = tier2(bytes);

        println!(
            "{:>10}  {:>9}  │  {:>9} {:>3}r {:>8}  │  {:>9} {:>3}r {:>8}  │  {:>9} {:>4}r {:>9}",
            n,
            fmt_bytes(file),
            fmt_bytes(r0.bytes),
            r0.reqs,
            ms(r0.time),
            fmt_bytes(r1.bytes),
            r1.reqs,
            ms(r1.time),
            fmt_bytes(r2.bytes),
            r2.reqs,
            ms(r2.time),
        );
        // Sanity: Tier-0 finds the unsatisfiable class; Tier-1/2 also see the
        // instance clashes. (Printed once, on the largest run, to confirm.)
        eprintln!(
            "    ({total} triples · found: T0={} T1={} T2={} incoherent points · Tier-0 read {:.4}% of the file)",
            r0.found,
            r1.found,
            r2.found,
            100.0 * r0.bytes as f64 / file as f64,
        );
    }
}
