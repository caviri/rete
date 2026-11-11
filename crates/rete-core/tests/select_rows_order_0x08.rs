//! A result handle over a `0x08` file sorts on ids: an `ORDER BY` whose key is a
//! plain variable of an order-exact group decodes no key, so the handle costs
//! no term, and a page costs the terms of the page. One test in this file: the
//! decode counter is process-wide.

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{eval_query, eval_select_rows, terms_decoded, QueryOpts, QueryOutput, Rete};

fn dataset() -> Vec<RawQuad> {
    let mut q = Vec::new();
    for i in 0..4000u32 {
        // Subjects are not inserted in sorted order.
        let s = format!("<http://example.org/s/{:05}>", (i * 7919) % 4000);
        q.push((
            s.clone(),
            "<http://example.org/name>".into(),
            format!("\"name {i}\""),
            None,
        ));
        q.push((
            s,
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
            "<http://example.org/C>".into(),
            None,
        ));
    }
    q
}

fn full(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap() {
        QueryOutput::Select(_, rows) => rows.into_iter().map(|b| format!("{b:?}")).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn order_by_a_plain_variable_decodes_no_key_and_pages_are_slices() {
    let (v5, _) = assemble_dataset_with_opts(dataset(), false, false, None, |_, _| Vec::new());
    let bytes = rete_core::v06::write::convert_image(&v5, Default::default()).unwrap();
    let rete = Rete::open(&bytes).unwrap();
    for q in [
        "SELECT ?s WHERE { ?s a <http://example.org/C> } ORDER BY ?s",
        "SELECT ?s ?o WHERE { ?s <http://example.org/name> ?o } ORDER BY DESC(?s)",
        "SELECT DISTINCT ?s WHERE { ?s ?p ?o } ORDER BY ?s",
    ] {
        let before = terms_decoded();
        let h = eval_select_rows(&rete, q, QueryOpts::default()).unwrap();
        let handle = terms_decoded() - before;
        assert!(
            handle < 50,
            "{q}: the handle decoded {handle} terms; ids sort"
        );
        let want = full(&rete, q);
        assert_eq!(h.len(), want.len());
        let before = terms_decoded();
        let page = h.page(&rete, 100, 25);
        let used = terms_decoded() - before;
        assert_eq!(
            page.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
            want[100..125],
            "{q}"
        );
        assert!(used < 200, "{q}: a page of 25 decoded {used} terms");
        // The first, a middle and the last page; every slice.
        for (off, lim) in [(0, 25), (3999, 10), (4000, 5), (1234, 1)] {
            let got: Vec<String> = h
                .page(&rete, off, lim)
                .iter()
                .map(|b| format!("{b:?}"))
                .collect();
            let end = (off + lim).min(want.len());
            assert_eq!(got, want[off.min(want.len())..end], "{q} {off}+{lim}");
        }
    }
}
