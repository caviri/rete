//! Class clustering, the paths the planner takes only over a big file: the
//! per-row probes of a type-scoped join (bounded windows, batched tile reads)
//! with the plan limits shrunk so a graph small enough for a test takes them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::write::{convert_image, WriteOpts};
use crate::file::Rete;
use crate::ingest::{assemble_dataset_opts, AssembleOptions, GraphLayout, RawQuad};
use crate::reader::RangeReader;
use crate::sparql::{eval_query, QueryOutput};
use crate::write_opts::ClusterOpts;

const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

fn subject(i: u32) -> String {
    iri(&format!("r/{:07}", (i as u64 * 7919) % 1_000_003))
}

fn dataset(n: u32) -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..n {
        let s = subject(i);
        let class = match i % 100 {
            0..=39 => Some("A"),
            40..=64 => Some("B"),
            65..=79 => Some("C"),
            80..=84 => Some("E"),
            85 => Some("D"),
            _ => None,
        };
        if let Some(c) = class {
            q.push((s.clone(), TYPE.into(), iri(&format!("C{c}")), None));
        }
        q.push((s.clone(), iri("name"), format!("\"name of {i}\""), None));
        q.push((s.clone(), iri("v"), iri(&format!("o/{}", i % 50)), None));
        q.push((s.clone(), iri("cites"), subject((i * 31 + 7) % n), None));
        q.push((
            s.clone(),
            iri("year"),
            format!("\"{}\"", 1990 + i % 30),
            None,
        ));
    }
    q
}

struct Rec {
    data: Vec<u8>,
    reads: AtomicU64,
    bytes: AtomicU64,
}

impl RangeReader for Rec {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(len, Ordering::Relaxed);
        self.data
            .get(offset as usize..(offset + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
    fn concurrency(&self) -> usize {
        4
    }
}

fn lazy(data: &[u8]) -> (Rete, Arc<Rec>) {
    let rec = Arc::new(Rec {
        data: data.to_vec(),
        reads: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
    });
    let r = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.reads.store(0, Ordering::Relaxed);
    rec.bytes.store(0, Ordering::Relaxed);
    (r, rec)
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn files() -> (Vec<u8>, Vec<u8>) {
    let mut o = AssembleOptions::sectioned();
    o.graph_layout = GraphLayout::Quad;
    let (v5, _) = assemble_dataset_opts(dataset(60_000), &o, |_, _| Vec::new());
    let plain = convert_image(&v5, WriteOpts::default().with_cluster(None)).unwrap();
    let clustered = convert_image(
        &v5,
        WriteOpts::default().with_cluster(Some(ClusterOpts {
            min_subjects: 100,
            ..ClusterOpts::default()
        })),
    )
    .unwrap();
    (plain, clustered)
}

/// A join over one class's subjects, probed per row, answers the same and
/// reads far fewer bytes from the clustered file; a class that is not listed
/// still answers.
#[test]
fn a_probed_type_scoped_join_reads_one_run() {
    let (plain, clustered) = files();
    eprintln!("plain {} B, clustered {} B", plain.len(), clustered.len());
    assert!(clustered.len() < plain.len() * 12 / 10);
    let qs = [
        // D: 1% of the subjects, listed.
        format!("SELECT ?s ?o WHERE {{ ?s {TYPE} <http://ex/CD> . ?s <http://ex/name> ?o }}"),
        format!("SELECT ?s ?y WHERE {{ ?s {TYPE} <http://ex/CD> . ?s <http://ex/year> ?y . ?s <http://ex/v> ?v }}"),
        // Two classes: the probes move between runs.
        format!("SELECT ?s ?o WHERE {{ ?s {TYPE} ?c . ?s <http://ex/name> ?o FILTER(?c = <http://ex/CD> || ?c = <http://ex/CA>) }}"),
        format!("SELECT ?s ?o WHERE {{ ?s {TYPE} <http://ex/CZ> . ?s <http://ex/name> ?o }}"),
        format!("SELECT ?a ?o WHERE {{ ?a <http://ex/cites> ?s . ?s {TYPE} <http://ex/CD> . ?s <http://ex/name> ?o }}"),
    ];
    for q in &qs {
        let (lp, rp) = lazy(&plain);
        let (lc, rc) = lazy(&clustered);
        let (want, got) =
            crate::bgp::with_plan_limits(0.0, f64::MAX, f64::MAX, || (rows(&lp, q), rows(&lc, q)));
        assert_eq!(got, want, "{q}");
        let (bp, bc) = (
            rp.bytes.load(Ordering::Relaxed),
            rc.bytes.load(Ordering::Relaxed),
        );
        eprintln!(
            "{q}\n  plain {} reads {bp} B, clustered {} reads {bc} B, {} rows",
            rp.reads.load(Ordering::Relaxed),
            rc.reads.load(Ordering::Relaxed),
            got.len()
        );
        if q.contains("CD> . ?s <http://ex/name>") && !q.contains("cites") {
            assert!(bc * 2 < bp, "{bc} against {bp} bytes");
        }
    }
}
