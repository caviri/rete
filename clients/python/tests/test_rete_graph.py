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
