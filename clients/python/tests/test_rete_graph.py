"""End-to-end tests over every open path: bytes, file, HTTP range, custom
reader — all four must answer identically."""

from __future__ import annotations

import os

import pytest

import rete_graph as rete

KNOWS_Q = "SELECT ?s ?o WHERE { ?s <http://example.org/knows> ?o }"
LABELS_Q = "SELECT ?label WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label }"


def test_build_and_open_bytes(rete_bytes):
    g = rete.open(rete_bytes)
    assert g.quads == 6
    assert g.info()["quads"] == 6
    assert len(g.content_hash()) == 32

    rows = g.query(KNOWS_Q)
    assert len(rows) == 1
    assert rows[0]["s"].kind == "iri"
    assert rows[0]["s"].value == "http://example.org/bob"
    assert rows[0]["o"].value == "http://example.org/alice"


def test_term_parsing(rete_bytes):
    g = rete.open(rete_bytes)
    labels = {t["label"].value: t["label"] for t in g.query(LABELS_Q)}

    quoted = labels['Alice "the researcher"']  # N-Triples escapes resolved
    assert quoted.kind == "literal" and quoted.lang is None

    bob = labels["Bob"]
    assert bob.lang == "en"
    assert bob.n3 == '"Bob"@en'

    (age_row,) = g.query(
        "SELECT ?age WHERE { <http://example.org/alice> <http://example.org/age> ?age }"
    )
    age = age_row["age"]
    assert age.datatype == "http://www.w3.org/2001/XMLSchema#integer"
    assert age.to_python() == 42


def test_ask_and_construct(rete_bytes):
    g = rete.open(rete_bytes)
    assert g.query("ASK { ?s <http://example.org/knows> ?o }") is True
    assert g.query("ASK { ?s <http://example.org/hates> ?o }") is False

    triples = g.query(
        "CONSTRUCT { ?o <http://example.org/knownBy> ?s } "
        "WHERE { ?s <http://example.org/knows> ?o }"
    )
    assert len(triples) == 1
    s, p, o = triples[0]
    assert p.value == "http://example.org/knownBy"
    assert s.value == "http://example.org/alice"


def test_open_path(tmp_path, rete_bytes):
    path = tmp_path / "example.rete"
    path.write_bytes(rete_bytes)
    g = rete.open(path)
    assert g.query(KNOWS_Q) == rete.open(rete_bytes).query(KNOWS_Q)


def test_open_url_lazy(serve_bytes, rete_bytes):
    url = serve_bytes(rete_bytes)
    g = rete.open(url)
    assert g.query(KNOWS_Q) == rete.open(rete_bytes).query(KNOWS_Q)

    stats = g.stats()
    assert stats["fileLength"] == len(rete_bytes)
    assert stats["requests"] >= 1
    assert 0 < stats["bytes"]


def test_open_custom_reader(rete_bytes):
    class MemReader:
        def __init__(self, data):
            self.data = data
            self.calls = 0

        def len(self):
            return len(self.data)

        def read_at(self, offset, length):
            self.calls += 1
            return self.data[offset : offset + length]

    r = MemReader(rete_bytes)
    g = rete.open(reader=r)
    assert g.query(KNOWS_Q) == rete.open(rete_bytes).query(KNOWS_Q)
    assert r.calls >= 1


SHAPES_PREFIX = """\
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
"""


def test_shacl_validation(rete_bytes, serve_bytes):
    g = rete.open(rete_bytes)

    labeled = SHAPES_PREFIX + """
ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path rdfs:label ; sh:minCount 1 ] .
"""
    report = g.shacl(labeled)
    assert report["conforms"] is True
    assert report["results"] == []

    emailed = SHAPES_PREFIX + """
ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:email ; sh:minCount 1 ] .
"""
    report = g.shacl(emailed)
    assert report["conforms"] is False
    assert len(report["results"]) == 2  # alice and bob both lack ex:email

    ttl = g.shacl(emailed, format="ttl")
    assert "ValidationReport" in ttl

    # Lazy path: same verdicts over HTTP range reads.
    remote = rete.open(serve_bytes(rete_bytes))
    assert remote.shacl(labeled)["conforms"] is True
    assert remote.shacl(emailed)["conforms"] is False


def test_schema_and_search(rete_bytes):
    g = rete.open(rete_bytes)
    classes = dict(g.schema()["classes"])
    assert classes.get("http://example.org/Person") == 2
    # prefix_search needs the pyramid label index; empty results are fine for
    # a synthetic micro-graph, but the call itself must work end to end.
    assert isinstance(g.prefix_search("Ali", limit=5), list)


def test_query_error_is_python_error(rete_bytes):
    g = rete.open(rete_bytes)
    with pytest.raises(ValueError):
        g.query("SELECT WHERE this is not sparql")


def test_build_rejects_garbage():
    with pytest.raises(ValueError):
        rete.build("")


def test_build_from_rdflib_graph():
    rdflib = pytest.importorskip("rdflib")
    g = rdflib.Graph()
    alice = rdflib.URIRef("http://example.org/alice")
    g.add((alice, rdflib.RDFS.label, rdflib.Literal("Alice")))
    g.add((alice, rdflib.URIRef("http://example.org/age"), rdflib.Literal(42)))

    rg = rete.open(rete.build(g))
    assert rg.quads == 2
    (row,) = rg.query(
        "SELECT ?age WHERE { <http://example.org/alice> <http://example.org/age> ?age }"
    )
    assert row["age"].to_python() == 42


def test_build_from_rdflib_dataset_keeps_named_graphs():
    rdflib = pytest.importorskip("rdflib")
    ds = rdflib.Dataset()
    alice = rdflib.URIRef("http://example.org/alice")
    named = rdflib.URIRef("http://example.org/graph1")
    ds.add((alice, rdflib.RDFS.label, rdflib.Literal("Alice")))  # default graph
    ds.graph(named).add(
        (alice, rdflib.URIRef("http://example.org/inGraph"), rdflib.Literal("yes"))
    )

    rg = rete.open(rete.build(ds))
    assert rg.quads == 2
    # The named graph survives; rdflib's synthetic default-graph label must not.
    assert rg.graph_names() == ["http://example.org/graph1"]
    assert rg.query("ASK { <http://example.org/alice> ?p ?o }") is True


def test_query_df(rete_bytes):
    pandas = pytest.importorskip("pandas")
    g = rete.open(rete_bytes)
    df = g.query_df(LABELS_Q)
    assert isinstance(df, pandas.DataFrame)
    assert set(df["label"]) == {'Alice "the researcher"', "Bob"}


def test_builder_end_to_end(tmp_path, nt_text):
    builder = (
        rete.Builder()
        .add(nt_text)
        .card(
            title="Tiny people graph",
            description="Two people who know each other.",
            license="CC0-1.0",
            source="https://example.org/people",
            created="2026-07-16",
            example_queries=[KNOWS_Q],
            keywords=["people", "demo"],
        )
        .text_index()
        .pyramid(algo="louvain")
    )

    data = builder.run()
    assert builder.stats["statements"] == 6
    assert builder.stats["terms"] > 0
    assert builder.run() is data  # cached: same object, no rebuild

    g = rete.open(data)
    card = g.card()
    assert card["title"] == "Tiny people graph"
    assert card["license"] == "CC0-1.0"
    assert card["example_queries"] == [KNOWS_Q]
    # `keywords` is a rete-defined field: it routes to the card's top level,
    # never into the `extra` bag (this client writes the card verbatim).
    assert card["keywords"] == ["people", "demo"]
    assert "extra" not in card
    # Counts + format_version are stamped automatically at build time.
    assert card["quad_count"] == 6
    assert card["term_count"] == g.terms
    assert card["format_version"] >= 1

    # The opt-in text index was actually built and is queryable.
    assert "http://example.org/alice" in g.text_search("researcher")

    # export() writes the same immutable image; a lazy reopen reads the card
    # through the ranged metadata path.
    path = builder.export(tmp_path / "people.rete")
    g2 = rete.open(path)
    assert g2.content_hash() == g.content_hash()
    assert g2.card()["title"] == "Tiny people graph"


def test_builder_no_pyramid(nt_text):
    g = rete.Builder().add(nt_text).pyramid(False).graph()
    assert g.info()["pyramidLevels"] == 0
    assert g.query("ASK { ?s ?p ?o }") is True  # still fully queryable


def test_builder_reruns_after_config_change(nt_text):
    builder = rete.Builder().add(nt_text)
    first = rete.open(builder.run()).quads
    builder.add("<http://example.org/x> <http://example.org/p> <http://example.org/y> .")
    assert rete.open(builder.run()).quads == first + 1


def test_builder_requires_sources():
    with pytest.raises(ValueError):
        rete.Builder().run()


def test_builder_rejects_unknown_pyramid_algo(nt_text):
    with pytest.raises(ValueError):
        rete.Builder().add(nt_text).pyramid(algo="voronoi").run()


def test_no_card_means_none(rete_bytes):
    assert rete.open(rete_bytes).card() is None
    assert rete.open(rete_bytes).examples() == []


def test_embedded_example_queries(tmp_path, nt_text):
    builder = (
        rete.Builder()
        .add(nt_text)
        .card(title="With examples", example_queries=[LABELS_Q])  # legacy strings
        .example(
            KNOWS_Q,
            title="Who knows whom?",
            question="Which people know each other?",
        )
        .example("ASK { ?s ?p ?o }", title="Anything at all?")
    )
    g = builder.graph()

    examples = g.examples()
    assert len(examples) == 3  # two rich + one legacy
    rich = examples[0]
    assert rich["title"] == "Who knows whom?"
    assert rich["question"] == "Which people know each other?"
    assert rich["sparql"] == KNOWS_Q
    assert rich["id"] == "ex-1" and rich["tier"] == "index"
    assert examples[2] == {"sparql": LABELS_Q}  # legacy entry, sparql only

    # The whole point: every example embedded in the file actually runs.
    for example in examples:
        g.query(example["sparql"])

    # And they survive export + a lazy reopen (ranged card read).
    path = builder.export(tmp_path / "examples.rete")
    assert rete.open(path).examples() == examples


# --- lazy quad dump -------------------------------------------------------


def test_iter_quads_round_trips_the_default_graph(rete_bytes):
    g = rete.open(rete_bytes)
    quads = list(g.iter_quads())

    assert len(quads) == g.quads == 6
    assert all(graph is None for *_spo, graph in quads)

    # Differential check against an independent engine path: the same triples
    # the BGP evaluator sees, in the same canonical token form.
    rows = g.query_raw("SELECT ?s ?p ?o WHERE { ?s ?p ?o }")["rows"]
    assert sorted(tuple(spo) for *spo, _ in quads) == sorted(
        (r["s"], r["p"], r["o"]) for r in rows
    )

    # Tokens are the lossless N-Triples surface form, so they re-parse.
    by_predicate = {p: o for _s, p, o, _g in quads}
    label = "<http://www.w3.org/2000/01/rdf-schema#label>"
    labels = {rete.Term.parse(o).value for _s, p, o, _g in quads if p == label}
    assert labels == {'Alice "the researcher"', "Bob"}
    assert rete.Term.parse(by_predicate["<http://example.org/age>"]).to_python() == 42


def test_iter_quads_covers_blank_node_and_quoted_triple_subjects():
    """The walk is driven by subject ids, and blank nodes and RDF-star quoted
    triples share that id space with IRIs — none of them may be skipped."""
    exotic = (
        "_:b0 <http://example.org/p> <http://example.org/o> .\n"
        "_:b0 <http://example.org/q> _:b1 .\n"
        "<http://example.org/s> <http://example.org/p> _:b1 .\n"
        "<<<http://example.org/s> <http://example.org/p> <http://example.org/o>>>"
        ' <http://example.org/certainty> "0.9" .\n'
        "<http://example.org/s> <http://example.org/p> <http://example.org/o> .\n"
    )
    g = rete.open(rete.build(exotic))
    quads = sorted(g.iter_quads())

    assert len(quads) == g.quads == 5
    kinds = {rete.Term.parse(s).kind for s, _p, _o, _g in quads}
    assert kinds == {"iri", "bnode", "triple"}
    rows = g.query_raw("SELECT ?s ?p ?o WHERE { ?s ?p ?o }")["rows"]
    assert sorted((r["s"], r["p"], r["o"]) for r in rows) == [q[:3] for q in quads]


def test_iter_quads_preserves_named_graphs(nq_bytes):
    g = rete.open(nq_bytes)
    quads = list(g.iter_quads())

    assert len(quads) == g.quads == 5
    by_graph = {}
    for s, p, o, graph in quads:
        by_graph.setdefault(graph, set()).add((s, p, o))
    assert set(by_graph) == {
        None,
        "<http://example.org/g1>",
        "<http://example.org/g2>",
    }
    assert len(by_graph[None]) == 2
    assert len(by_graph["<http://example.org/g1>"]) == 2
    assert by_graph["<http://example.org/g2>"] == {
        (
            "<http://example.org/bob>",
            "<http://example.org/knows>",
            "<http://example.org/alice>",
        )
    }

    # Scoping: the default graph alone, one named graph alone (bare IRI or
    # `<token>`), and an IRI that is not in the file.
    assert {q[3] for q in g.iter_quads(rete.DEFAULT_GRAPH)} == {None}
    assert len(list(g.iter_quads(rete.DEFAULT_GRAPH))) == 2
    named = list(g.iter_quads("http://example.org/g1"))
    assert len(named) == 2
    assert named == list(g.iter_quads("<http://example.org/g1>"))
    assert list(g.iter_quads("http://example.org/nope")) == []


def test_iter_quads_filters_are_the_full_dump_filtered(nq_bytes):
    """A filtered dump must return exactly the quads the unfiltered dump
    returns, filtered — the correctness half of the pruning it does.

    The saving comes from *not fetching* index tiles a synopsis rejects, and a
    pruning bug loses rows without erroring, so the expectation here is computed
    from the unfiltered walk rather than written out by hand.
    """
    g = rete.open(nq_bytes)
    everything = list(g.iter_quads())

    knows = "http://example.org/knows"
    assert sorted(g.iter_quads(predicate=knows)) == sorted(
        q for q in everything if q[1] == f"<{knows}>"
    )
    # A bare IRI, an `<iri>` token and a Term are the same filter.
    assert sorted(g.iter_quads(predicate=f"<{knows}>")) == sorted(
        g.iter_quads(predicate=knows)
    )
    assert sorted(g.iter_quads(predicate=rete.Term("iri", knows))) == sorted(
        g.iter_quads(predicate=knows)
    )

    alice = "http://example.org/alice"
    assert sorted(g.iter_quads(subject=alice)) == sorted(
        q for q in everything if q[0] == f"<{alice}>"
    )
    assert sorted(g.iter_quads(object=alice)) == sorted(
        q for q in everything if q[2] == f"<{alice}>"
    )

    # Filters compose with each other and with the graph scope.
    assert sorted(g.iter_quads(subject=alice, predicate=knows)) == sorted(
        q for q in everything if q[0] == f"<{alice}>" and q[1] == f"<{knows}>"
    )
    assert sorted(g.iter_quads("http://example.org/g1", predicate=knows)) == sorted(
        q
        for q in everything
        if q[1] == f"<{knows}>" and q[3] == "<http://example.org/g1>"
    )

    # A term the file does not contain yields nothing — not everything.
    assert list(g.iter_quads(predicate="http://example.org/nope")) == []
    assert list(g.iter_quads(subject="http://example.org/nobody")) == []
    assert list(g.iter_quads(object='"no such literal"')) == []


def test_to_nquads_takes_the_same_filters(nq_bytes):
    import io

    g = rete.open(nq_bytes)
    out = io.StringIO()
    written = g.to_nquads(out, predicate="http://example.org/knows")
    lines = [line for line in out.getvalue().splitlines() if line]
    assert written == len(lines)
    assert all("<http://example.org/knows>" in line for line in lines)
    assert len(lines) == len(list(g.iter_quads(predicate="http://example.org/knows")))


def test_a_filtered_dump_over_a_remote_graph_agrees_and_costs_no_more(
    serve_bytes, multiblock_rete_bytes
):
    """Over a lazily range-read remote graph, a filtered dump must return
    exactly the unfiltered dump filtered, and fetch no more than it.

    Deliberately *not* asserting a byte ratio here. The filter prunes the index,
    not the dictionary, and on this fixture — 200 000 quads whose objects are
    200 000 distinct literals — resolving a fifth of the rows still needs a
    fifth of the object dictionary, while the physical counter reports
    block-aligned fetches over a 3.4 MB file. Measured: 1,048,576 B for the
    whole dump and the same for the slice; on a 17 MB build of the same shape,
    3,932,160 B vs 3,145,728 B. The byte proof belongs where it can be made
    exact, and is: `a_predicate_scoped_dump_fetches_less_than_the_graph` in
    rete-core. What this guards is that the client wires the filter to the
    engine's scan at all, over the remote path, without losing rows.
    """
    url = serve_bytes(multiblock_rete_bytes)

    whole = rete.open(url)
    everything = list(whole.iter_quads())
    assert len(everything) == whole.quads
    whole_bytes = whole.stats()["bytes"]

    sliced = rete.open(url)
    rows = list(sliced.iter_quads(predicate="http://example.org/p0"))
    # Sorted, not positional: a filtered walk streams in the ROUTED
    # permutation's order (a bound predicate routes to POS), so the rows are the
    # same set in a different order. See `iter_quads`.
    assert sorted(rows) == sorted(
        q for q in everything if q[1] == "<http://example.org/p0>"
    )
    assert 0 < len(rows) < whole.quads
    assert sliced.stats()["bytes"] <= whole_bytes

    one = rete.open(url)
    assert sorted(one.iter_quads(subject="http://example.org/s7")) == sorted(
        q for q in everything if q[0] == "<http://example.org/s7>"
    )
    assert one.stats()["bytes"] <= whole_bytes


def test_iter_quads_batching_is_invisible(big_rete_bytes):
    """Any batch size yields the same quads in the same order — batching is an
    implementation detail of *how much* is resolved per call, never of what."""
    g = rete.open(big_rete_bytes)
    reference = list(g.iter_quads(batch_size=10_000))
    assert len(reference) == g.quads
    for batch_size in (1, 7, 999):
        assert list(g.iter_quads(batch_size=batch_size)) == reference


def test_iter_quads_memory_is_bounded_by_the_batch(big_rete_bytes):
    """The point of the whole exercise: streaming N quads must not cost N
    quads of RAM. tracemalloc counts Python allocations exactly, so this is a
    measurement, not a vibe."""
    import tracemalloc

    g = rete.open(big_rete_bytes)
    total = g.quads

    tracemalloc.start()
    try:
        tracemalloc.reset_peak()
        assert sum(1 for _ in g.iter_quads(batch_size=1_000)) == total
        small_batch = tracemalloc.get_traced_memory()[1]

        tracemalloc.reset_peak()
        assert sum(1 for _ in g.iter_quads(batch_size=20_000)) == total
        big_batch = tracemalloc.get_traced_memory()[1]

        tracemalloc.reset_peak()
        materialized = list(g.iter_quads(batch_size=1_000))
        whole_list = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()
    del materialized

    # Streaming peaks at one batch; the list peaks at the whole graph. With
    # 40 000 quads vs a 1 000-quad batch the gap is ~40×, so 5× is a wide
    # margin that still fails loudly if laziness ever regresses.
    assert small_batch * 5 < whole_list, (small_batch, whole_list)
    # And the bound really is the batch: 20× the batch, ~20× the peak.
    assert big_batch > small_batch * 4, (small_batch, big_batch)


def test_iter_quads_over_a_lazy_remote_graph_is_ranged(serve_bytes, multiblock_rete_bytes):
    """A lazy (HTTP range) open must stay ranged all the way through a dump —
    the walk faults index tiles and dictionary chunks as it needs them, so it
    never degenerates into "download the file, then iterate"."""
    import itertools

    url = serve_bytes(multiblock_rete_bytes)

    peek = rete.open(url)
    opened_bytes = peek.stats()["bytes"]
    first = list(itertools.islice(peek.iter_quads(), 1))
    assert len(first) == 1 and first[0][0].startswith("<http://example.org/s")
    peeked_bytes = peek.stats()["bytes"]

    full = rete.open(url)
    assert sum(1 for _ in full.iter_quads()) == full.quads == 200_000
    dumped_bytes = full.stats()["bytes"]

    # Never a download: 200 000 quads come out of a fraction of the file.
    assert dumped_bytes < len(multiblock_rete_bytes) / 2, dumped_bytes
    # And the walk is incremental — one quad costs no more than all of them,
    # and no more than the open that preceded it plus what it actually read.
    assert opened_bytes <= peeked_bytes <= dumped_bytes


def test_iter_quads_early_exit_stops_the_walk(big_rete_bytes):
    """Abandoning the generator must abandon the scan: no thread to join, no
    background work, nothing left running."""
    import itertools

    g = rete.open(big_rete_bytes)
    walk = g.iter_quads(batch_size=16)
    assert len(list(itertools.islice(walk, 3))) == 3
    walk.close()  # a plain generator: closing it is the whole cleanup

    # `break` out of a for-loop is the same thing, and the graph stays usable.
    for i, _quad in enumerate(g.iter_quads()):
        if i == 2:
            break
    assert sum(1 for _ in g.iter_quads()) == g.quads


def test_iter_quads_matches_across_every_open_path(rete_bytes, tmp_path, serve_bytes):
    path = tmp_path / "example.rete"
    path.write_bytes(rete_bytes)

    class MemReader:
        def __init__(self, data):
            self.data = data

        def len(self):
            return len(self.data)

        def read_at(self, offset, length):
            return self.data[offset : offset + length]

    reference = sorted(rete.open(rete_bytes).iter_quads())
    assert sorted(rete.open(path).iter_quads()) == reference
    assert sorted(rete.open(serve_bytes(rete_bytes)).iter_quads()) == reference
    assert sorted(rete.open(reader=MemReader(rete_bytes)).iter_quads()) == reference


def test_to_nquads_round_trips_through_a_file(tmp_path, nq_bytes):
    g = rete.open(nq_bytes)
    out = tmp_path / "dump.nq"

    assert g.to_nquads(out) == g.quads
    text = out.read_text(encoding="utf-8")
    assert len(text.splitlines()) == 5
    assert text.endswith(" .\n")

    # The real proof: rebuild from the dump and get the same graph back.
    rebuilt = rete.open(rete.build(text, "nq"))
    assert rebuilt.quads == g.quads
    assert sorted(rebuilt.iter_quads()) == sorted(g.iter_quads())
    assert rebuilt.graph_names() == g.graph_names()


def test_to_nquads_accepts_text_and_binary_streams(nq_bytes, tmp_path):
    import gzip
    import io as _io

    g = rete.open(nq_bytes)

    text_sink = _io.StringIO()
    assert g.to_nquads(text_sink) == 5
    binary_sink = _io.BytesIO()
    assert g.to_nquads(binary_sink) == 5
    assert binary_sink.getvalue().decode("utf-8") == text_sink.getvalue()
    assert not text_sink.closed  # a caller's stream is never closed for them

    gz = tmp_path / "dump.nq.gz"
    with gzip.open(gz, "wb") as fh:
        g.to_nquads(fh)
    with gzip.open(gz, "rt", encoding="utf-8") as fh:
        assert fh.read() == text_sink.getvalue()


def test_to_nquads_scopes_and_escapes(rete_bytes, nq_bytes):
    # Default-graph-only scoping writes plain N-Triples lines (no graph term).
    g = rete.open(nq_bytes)
    only_default = _io_text(g, rete.DEFAULT_GRAPH)
    assert len(only_default.splitlines()) == 2
    assert "http://example.org/g1" not in only_default

    # Escapes and language tags survive verbatim into the serialization.
    text = _io_text(rete.open(rete_bytes), None)
    assert '"Alice \\"the researcher\\""' in text
    assert '"Bob"@en' in text


def _io_text(graph, scope):
    import io as _io

    sink = _io.StringIO()
    graph.to_nquads(sink, graph=scope)
    return sink.getvalue()


def test_to_nquads_streams_a_big_graph_in_bounded_memory(big_rete_bytes, tmp_path):
    import tracemalloc

    g = rete.open(big_rete_bytes)
    out = tmp_path / "big.nq"

    def peak_writing(batch_size):
        tracemalloc.reset_peak()
        assert g.to_nquads(out, batch_size=batch_size) == g.quads
        return tracemalloc.get_traced_memory()[1]

    tracemalloc.start()
    try:
        small_batch = peak_writing(1_000)
        big_batch = peak_writing(20_000)
    finally:
        tracemalloc.stop()

    written = out.stat().st_size
    assert len(out.read_text(encoding="utf-8").splitlines()) == g.quads
    # Peak Python memory tracks the batch, not the 5 MB serialization.
    assert small_batch < written / 3, (small_batch, written)
    assert big_batch > small_batch * 3, (small_batch, big_batch)


@pytest.mark.skipif(
    not os.environ.get("RETE_REMOTE_URL"),
    reason="set RETE_REMOTE_URL to a public .rete URL for the live smoke test",
)
def test_live_remote_smoke():
    g = rete.open(os.environ["RETE_REMOTE_URL"])
    assert g.quads > 0
    rows = g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3")
    assert 0 < len(rows) <= 3
    assert g.stats()["bytes"] < g.stats()["fileLength"]  # lazy, not a download
