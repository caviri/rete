//! A result handle (`eval_select_rows`): the SELECT runs to dictionary ids and
//! a page of it is decoded on demand. The pages are the rows `eval_query`
//! returns, and a page costs the chunks of its own terms, not of the result.
//! Counted at the reader, on a lazy open.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{
    eval_query, eval_select_rows, QueryOpts, QueryOutput, RangeReader, Rete, SliceReader,
};

/// The bytes every read asked for.
#[derive(Clone)]
struct Bytes {
    data: Arc<Vec<u8>>,
    asked: Arc<Mutex<u64>>,
}

impl Bytes {
    fn new(data: Vec<u8>) -> Self {
        Bytes {
            data: Arc::new(data),
            asked: Arc::new(Mutex::new(0)),
        }
    }
    fn take(&self) -> u64 {
        std::mem::take(&mut *self.asked.lock().unwrap())
    }
}

impl RangeReader for Bytes {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        *self.asked.lock().unwrap() += len;
        SliceReader::new(&self.data).read_at(offset, len)
    }
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        *self.asked.lock().unwrap() += ranges.iter().map(|r| r.1).sum::<u64>();
        let r = SliceReader::new(&self.data);
        ranges.iter().map(|&(o, l)| r.read_at(o, l)).collect()
    }
}

/// 20,000 subjects of 3 rows each over long literals that do not compress
/// (the dictionary is a few MB), a type per subject, and a second title on
/// every 7th subject so that the OPTIONAL leaves unbound values.
fn quads() -> Vec<RawQuad> {
    let iri = |s: String| format!("<http://example.org/{s}>");
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut noise = move || -> String {
        (0..40)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(b'a' + (state % 26) as u8)
            })
            .collect()
    };
    let mut q: Vec<RawQuad> = Vec::new();
    for k in 0..20_000 {
        let s = iri(format!("s/{k:05}"));
        q.push((
            s.clone(),
            iri("type".into()),
            iri(format!("T{}", k % 5)),
            None,
        ));
        q.push((
            s.clone(),
            iri("name".into()),
            format!("\"Name {k:05} {}\"", noise()),
            None,
        ));
        q.push((
            s.clone(),
            iri("note".into()),
            format!("\"Note {k:05} {}\"@en", noise()),
            None,
        ));
        if k % 7 == 0 {
            q.push((
                s,
                iri("alt".into()),
                format!("\"Alt {k:05} {}\"", noise()),
                None,
            ));
        }
    }
    q
}

fn image() -> Vec<u8> {
    let (v5, _) = assemble_dataset_with_opts(quads(), false, false, None, |_, _| Vec::new());
    rete_core::v06::write::convert_image(&v5, Default::default()).unwrap()
}

/// What `eval_query` returns for `q`, row by row, in order.
fn rows(rete: &Rete, q: &str) -> (Vec<String>, Vec<String>) {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(vars, rows) => {
            (vars, rows.into_iter().map(|b| format!("{b:?}")).collect())
        }
        other => panic!("unexpected output {other:?}"),
    }
}

/// Every row of the handle, paged `step` rows at a time.
fn paged(rete: &Rete, q: &str, step: usize) -> (Vec<String>, Vec<String>) {
    let h = eval_select_rows(rete, q, QueryOpts::default()).unwrap_or_else(|e| panic!("{q}: {e}"));
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let page = h.page(rete, at, step);
        if page.is_empty() {
            break;
        }
        at += page.len();
        out.extend(page.into_iter().map(|b| format!("{b:?}")));
    }
    assert_eq!(out.len(), h.len(), "{q}: len() is the row count");
    (h.vars().to_vec(), out)
}

#[test]
fn the_pages_are_the_rows_of_eval_query() {
    let bytes = image();
    let rete = Rete::open(&bytes).unwrap();
    let ns = "PREFIX ex: <http://example.org/> ";
    for q in [
        "SELECT ?s ?o WHERE { ?s ex:name ?o }",
        "SELECT * WHERE { ?s ex:name ?o . OPTIONAL { ?s ex:alt ?a } }",
        "SELECT ?s ?a WHERE { ?s ex:type ex:T3 . OPTIONAL { ?s ex:alt ?a } }",
        "SELECT ?s ?o WHERE { ?s ex:name ?o } ORDER BY DESC(?s) LIMIT 50 OFFSET 7",
        "SELECT ?s ?o WHERE { ?s ex:note ?o } ORDER BY ?o",
        "SELECT DISTINCT ?t WHERE { ?s ex:type ?t }",
        "SELECT ?t (COUNT(*) AS ?n) WHERE { ?s ex:type ?t } GROUP BY ?t ORDER BY ?t",
        "SELECT ?s (STRLEN(?o) AS ?l) WHERE { ?s ex:name ?o FILTER(?s < ex:s/00100) }",
        "SELECT ?x WHERE { ?s ex:nothing ?x }",
    ] {
        let q = format!("{ns}{q}");
        let want = rows(&rete, &q);
        for step in [1, 7, 1000, usize::MAX / 2] {
            let got = paged(&rete, &q, step);
            if q.contains("SELECT *") {
                // `SELECT *`: eval_query names no variables.
                assert_eq!(got.1, want.1, "{q} (step {step})");
                assert_eq!(got.0, vec!["a", "o", "s"], "{q}");
            } else {
                assert_eq!(got, want, "{q} (step {step})");
            }
        }
    }
    // A window past the end is empty, and a window straddling it is cut.
    let h = eval_select_rows(
        &rete,
        "SELECT ?s WHERE { ?s <http://example.org/type> ?t }",
        QueryOpts::default(),
    )
    .unwrap();
    assert_eq!(h.len(), 20_000);
    assert!(h.page(&rete, 20_000, 10).is_empty());
    assert_eq!(h.page(&rete, 19_995, 10).len(), 5);
    // Not a SELECT: refused.
    assert!(eval_select_rows(&rete, "ASK { ?s ?p ?o }", QueryOpts::default()).is_err());
}

#[test]
fn a_page_decodes_the_chunks_of_its_own_terms() {
    let bytes = image();
    let rec = Bytes::new(bytes.clone());
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();

    let q = "SELECT ?s ?o WHERE { ?s <http://example.org/name> ?o }";
    // Today's way: every row's terms are decoded before the first is shown.
    let all = rows(&lazy, q);
    let whole = rec.take();
    assert_eq!(all.1.len(), 20_000);

    // A cold open again, so the cache does not answer for the first run.
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();
    let h = eval_select_rows(&lazy, q, QueryOpts::default()).unwrap();
    let ran = rec.take();
    let first = h.page(&lazy, 0, 25);
    let page = rec.take();
    assert_eq!(first.len(), 25);
    assert_eq!(
        first.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
        all.1[..25]
    );
    eprintln!(
        "whole result {whole} B; handle {ran} B, first page {page} B; file {} B",
        bytes.len()
    );
    assert!(
        (ran + page) * 3 < whole,
        "the handle and its first page read {} B, decoding every row {whole} B",
        ran + page
    );
    // A page deep in the result costs a page too.
    let last = h.page(&lazy, 19_990, 25);
    let deep = rec.take();
    assert_eq!(last.len(), 10);
    assert!(deep * 8 < whole, "the last page read {deep} B of {whole} B");

    // ORDER BY decodes the sort key, not the other columns: sorted by the
    // subject (a small dictionary section), the literals of the page only.
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();
    let q = "SELECT ?s ?o WHERE { ?s <http://example.org/name> ?o } ORDER BY DESC(?s)";
    let h = eval_select_rows(&lazy, q, QueryOpts::default()).unwrap();
    let sorted = h.page(&lazy, 0, 25);
    let used = rec.take();
    assert_eq!(
        sorted.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
        rows(&lazy, q).1[..25]
    );
    assert!(
        used * 3 < whole,
        "a sorted first page read {used} B, decoding every row {whole} B"
    );
}
