"""FILTER type-error diagnostics: ``Graph.last_warnings()`` and the envelope's
``warnings``. Row counts must not change — these are the query shapes an agent
reported as "CONTAINS / STRSTARTS / REGEX unsupported"."""

from __future__ import annotations

import pytest

import rete_graph as rete

TTL = """@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://ex.org/map/geneva-1572> rdfs:label "Geneva town plan"@en ; <http://ex.org/year> 1572 .
"""
P = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "

TABLE = [
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva")) }', 1, None),
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }', 0,
     ("CONTAINS", 1, "iri", "type-error")),
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(STR(?s), "geneva")) }', 1, None),
    ('SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(?y, "15")) }', 0,
     ("STRSTARTS", 1, "numeric", "type-error")),
    ('SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(STR(?y), "15")) }', 1, None),
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?s, "geneva")) }', 0,
     ("REGEX", 1, "iri", "type-error")),
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva"@fr)) }', 0,
     ("CONTAINS", 2, "language-mismatch", "type-error")),
    ('SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "geneva")) }', 0,
     ("CONTAINS", 2, "case-sensitive", "hint")),
]


@pytest.fixture(scope="module")
def graph():
    return rete.open(rete.build(TTL, "ttl"))


@pytest.mark.parametrize("query,rows,want", TABLE)
def test_reported_table(graph, query, rows, want):
    assert len(graph.query(P + query)) == rows
    warnings = graph.last_warnings()
    if want is None:
        assert warnings == []
        return
    assert len(warnings) == 1, warnings
    w = warnings[0]
    assert (w["function"], w["argument"], w["argKind"], w["severity"]) == want
    assert w["hint"] and w["message"]


def test_iri_warning_detail_and_reset(graph):
    graph.query(P + TABLE[1][0])
    (w,) = graph.last_warnings()
    assert w["sample"] == "<http://ex.org/map/geneva-1572>"
    assert "STR()" in w["hint"]
    assert w["count"] == 1
    # A copy, not internal state; the next query resets it.
    graph.last_warnings().clear()
    assert len(graph.last_warnings()) == 1
    graph.query(P + TABLE[0][0])
    assert graph.last_warnings() == []


def test_envelope_and_ask(graph):
    assert "warnings" not in graph.query_raw(P + TABLE[0][0])
    assert graph.query_raw(P + TABLE[1][0])["warnings"][0]["argKind"] == "iri"
    assert graph.query(P + 'ASK { ?s rdfs:label ?l FILTER(STRENDS(?s, "1572")) }') is False
    assert graph.last_warnings()[0]["function"] == "STRENDS"


def test_failing_query_leaves_no_stale_warnings(graph):
    graph.query(P + TABLE[1][0])
    with pytest.raises(ValueError, match="is not implemented"):
        graph.query("SELECT ?s WHERE { ?s ?p ?o FILTER(<http://ex.org/fn#m>(?o)) }")
    assert graph.last_warnings() == []
