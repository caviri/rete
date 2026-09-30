"""RDF 1.2 in the Python client: reading RDF 1.2 Turtle/TriG, writing the
RDF 1.2 triple-term surface, the card's quoted-triple signal, and the property
blank-node labelling must keep — two separate parses never share a label."""

from __future__ import annotations

import io
import re

import pytest

import rete_graph as rete

RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"

# A triple term; a reifier (rdf:reifies + ex:source); an annotation (the
# asserted triple + rdf:reifies + ex:since); a directional language literal.
TTL12 = """@prefix ex: <http://example.test/> .
ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .
ex:erin ex:knows ex:frank {| ex:since "2020" |} .
ex:gina ex:name "Gina"@en--ltr .
"""

TRIG12 = """@prefix ex: <http://example.test/> .
ex:g {
  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .
  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .
}
"""

# RDF-star Turtle: the same `<< … >>` bytes mean a quoted triple here.
TTL_STAR = """@prefix ex: <http://example.test/> .
ex:claim ex:states << ex:a ex:p ex:b >> .
"""

# Object-position quoted triple, nested in the object slot of another one.
NT_QUOTED = (
    "<http://ex/a> <http://ex/p> <http://ex/b> .\n"
    "<http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> .\n"
    "<http://ex/meta> <http://ex/about> "
    "<< <http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> >> .\n"
)

# Subject-position quoted triple: legal RDF-star, no RDF 1.2 spelling.
NT_SUBJECT = "<< <http://ex/a> <http://ex/p> <http://ex/b> >> <http://ex/src> <http://ex/wiki> .\n"

PLAIN = "<http://ex/a> <http://ex/p> <http://ex/b> .\n<http://ex/b> <http://ex/p> \"x\"@en .\n"

ANON = '[] <http://example.test/p> "x" .\n'
SUBJECTS = 'SELECT ?s WHERE { ?s <http://example.test/p> "x" }'


def _nq(g: rete.Graph, **kw) -> str:
    sink = io.StringIO()
    g.to_nquads(sink, **kw)
    return sink.getvalue()


def _quad_set(g: rete.Graph) -> set:
    return set(g.iter_quads())


# --- reading ------------------------------------------------------------------


def test_reads_rdf12_turtle():
    g = rete.open(rete.build(TTL12, "ttl", quoted_triple_syntax="rdf12"))
    assert g.quads == 7
    reifiers = g.query(f"SELECT ?r WHERE {{ ?r <{RDF}reifies> ?t }}")
    assert len(reifiers) == 2
    assert len({r["r"].value for r in reifiers}) == 2, "reifiers must be distinct"
    assert g.query(
        "ASK { <http://example.test/erin> <http://example.test/knows> <http://example.test/frank> }"
    ), "an annotated triple is asserted"


def test_reads_rdf12_trig_with_builder_and_suffix(tmp_path):
    path = tmp_path / "data.trig"
    path.write_text(TRIG12, encoding="utf-8")
    g = rete.Builder().add_file(path, quoted_triple_syntax="rdf12").graph()
    assert g.quads == 3
    assert g.graph_names() == ["http://example.test/g"]


def test_rdf_star_is_the_default_and_unchanged():
    # RDF 1.2 Turtle through the default reader is a loud error naming the flag.
    with pytest.raises(ValueError, match="rdf12"):
        rete.build(TTL12, "ttl")
    with pytest.raises(ValueError, match="quoted_triple_syntax"):
        rete.build(PLAIN, "nt", quoted_triple_syntax="rdf13")
    # N-Triples ignores the flag: identical bytes under both values.
    assert rete.build(PLAIN) == rete.build(PLAIN, quoted_triple_syntax="rdf-star")
    assert rete.build(PLAIN) == rete.build(PLAIN, quoted_triple_syntax="rdf12")
    # The same Turtle bytes are two graphs under the two readers.
    star = rete.open(rete.build(TTL_STAR, "ttl"))
    r12 = rete.open(rete.Builder().add(TTL_STAR, "ttl", quoted_triple_syntax="rdf12").run())
    assert star.quads == 1
    assert r12.quads == 2  # a reifier: _:r rdf:reifies <<( … )>> ; ex:states …


# --- writing ------------------------------------------------------------------


def test_to_nquads_writes_rdf12_triple_terms_by_default():
    g = rete.open(rete.build(NT_QUOTED))
    text = _nq(g)
    assert "<<( <http://ex/a> <http://ex/p> <http://ex/b> )>>" in text
    assert "<<( <http://ex/claim> <http://ex/states> <<( " in text, "nested terms respelled"
    assert "<< " not in text.replace("<<( ", "")
    # The stored RDF-star token is still there on request.
    star = _nq(g, quoted_triple_syntax="rdf-star")
    assert "<<<http://ex/a> <http://ex/p> <http://ex/b>>>" in star
    with pytest.raises(ValueError, match="quoted_triple_syntax"):
        _nq(g, quoted_triple_syntax="turtle")


def test_rdf12_round_trip_turtle_to_nquads_and_back():
    # Build RDF 1.2 Turtle, export RDF 1.2 N-Quads, rebuild: the same graph.
    first = rete.open(rete.build(TTL12, "ttl", quoted_triple_syntax="rdf12"))
    dump = _nq(first)
    assert "<<( " in dump
    again = rete.open(rete.build(dump, "nq"))
    assert _quad_set(again) == _quad_set(first)
    # …and so does the RDF-star spelling of the same dump.
    star = rete.open(rete.build(_nq(first, quoted_triple_syntax="rdf-star"), "nq"))
    assert _quad_set(star) == _quad_set(first)


def test_rdf12_dump_loads_in_an_independent_rdf12_parser():
    pyoxigraph = pytest.importorskip("pyoxigraph")
    g = rete.open(rete.build(NT_QUOTED))
    store = pyoxigraph.Store()
    store.load(_nq(g).encode(), format=pyoxigraph.RdfFormat.N_QUADS)
    assert len(store) == 3
    # The stored RDF-star token is exactly what #262 found current parsers refuse.
    with pytest.raises(SyntaxError):
        pyoxigraph.Store().load(
            _nq(g, quoted_triple_syntax="rdf-star").encode(),
            format=pyoxigraph.RdfFormat.N_QUADS,
        )


def test_subject_position_quoted_triple_is_refused_in_rdf12_only():
    g = rete.open(rete.build(NT_SUBJECT))
    with pytest.raises(ValueError, match="rdf-star"):
        _nq(g)
    assert _nq(g, quoted_triple_syntax="rdf-star").startswith("<<<http://ex/a>")


def test_a_graph_without_quoted_triples_is_written_identically():
    g = rete.open(rete.build(PLAIN))
    assert _nq(g) == _nq(g, quoted_triple_syntax="rdf-star")


# --- the card -----------------------------------------------------------------


def test_card_reports_quoted_triples_from_the_header():
    def card_of(text):
        data = rete.Builder().add(text).card(title="t").run()
        return rete.open(data).card()

    q = card_of(NT_QUOTED)["signals"]["quoted_triples"]
    assert q == {
        "present": True,
        "export_surfaces": ["rdf12", "rdf-star"],
        "export_default": "rdf12",
    }
    assert card_of(PLAIN)["signals"]["quoted_triples"] == {"present": False}
    # Measured, never stored: the file's own bytes do not carry it.
    assert b"quoted_triples" not in rete.Builder().add(NT_QUOTED).card(title="t").run()
    # A file with no card still has no card.
    assert rete.open(rete.build(NT_QUOTED)).card() is None


def test_card_signal_on_a_lazy_open(tmp_path):
    path = tmp_path / "q.rete"
    rete.Builder().add(NT_QUOTED).card(title="t").export(path)
    assert rete.open(str(path)).card()["signals"]["quoted_triples"]["present"] is True


# --- blank nodes ----------------------------------------------------------------


def _blank_subjects(data: bytes) -> list:
    return [r["s"].value for r in rete.open(data).query(SUBJECTS)]


def test_blank_nodes_of_separate_parses_stay_distinct():
    labels = []
    for syntax in ("rdf12", "rdf12", "rdf-star", "rdf-star"):
        labels += _blank_subjects(rete.build(ANON, "ttl", quoted_triple_syntax=syntax))
    assert len(labels) == 4, labels
    assert all(re.fullmatch(r"_:\S+", label) for label in labels), labels
    assert len(set(labels)) == 4, f"blank nodes collided: {labels}"

    # Merge the four documents into one store: still four subjects.
    merged = "".join(f'{label} <http://example.test/p> "x" .\n' for label in labels)
    assert len(set(_blank_subjects(rete.build(merged)))) == 4

    # Two parses queued in one Builder are two documents, too.
    both = rete.Builder().add(ANON, "ttl").add(ANON, "ttl", quoted_triple_syntax="rdf12").run()
    assert len(set(_blank_subjects(both))) == 2
