"""Query round-trips through every plane relay serves, and the quoted-triple
surface of CONSTRUCT/DESCRIBE.

The CONSTRUCT bug these pin (dev/client-audit/AUDIT.md, "relay
/sparql/{dataset}"): relay joined the engine's stored tokens verbatim, so a
quoted triple went out as the RDF-star surface ``<<s p o>>``. RDF 1.2
N-Triples parsers reject that line, and an RDF 1.2 Turtle parser (relay
serves the same bytes for ``text/turtle``) reads ``<< s p o >>`` as a
*reifier*, a different graph. `rete export` writes ``<<( s p o )>>`` by
default; relay now does the same.
"""
from __future__ import annotations

import json
from importlib.metadata import version
from urllib.parse import quote

import pyoxigraph as ox
import pytest

from conftest import ALICE, BOB, EX, KNOWS, NAME, PEOPLE, QUOTED_RDF12, QUOTED_STAR, STAR_SUBJECT

SELECT_NAME = f"SELECT ?n WHERE {{ {ALICE} {NAME} ?n }}"
CONSTRUCT_CLAIM = (
    f"CONSTRUCT {{ ?c <{EX}asserts> ?t }} WHERE {{ ?c <{EX}asserts> ?t }}"
)


def _sparql(client, dataset, query, **params):
    qs = "&".join([f"query={quote(query)}"] + [f"{k}={quote(v)}" for k, v in params.items()])
    return client.get(f"/sparql/{dataset}?{qs}")


def test_runs_against_rete_graph_0_3_3_or_later():
    major, minor, patch = (int(x) for x in version("rete-graph").split(".")[:3])
    assert (major, minor, patch) >= (0, 3, 3), version("rete-graph")


def test_select_round_trip_rest(client):
    r = client.post("/api/query", json={"dataset": PEOPLE, "query": SELECT_NAME})
    assert r.status_code == 200, r.text
    doc = r.json()
    assert doc["kind"] == "select"
    assert doc["table"] == [{"n": "Alice"}]
    assert doc["results"]["bindings"] == [{"n": {"type": "literal", "value": "Alice"}}]
    assert doc["warnings"] == []


def test_select_round_trip_protocol(client):
    r = _sparql(client, PEOPLE, SELECT_NAME)
    assert r.status_code == 200, r.text
    assert r.headers["content-type"].startswith("application/sparql-results+json")
    body = r.json()
    assert body == {
        "head": {"vars": ["n"]},
        "results": {"bindings": [{"n": {"type": "literal", "value": "Alice"}}]},
    }
    assert "x-rete-warnings" not in r.headers


def test_select_round_trip_protocol_post_forms(client):
    form = client.post(f"/sparql/{PEOPLE}", data={"query": SELECT_NAME})
    raw = client.post(f"/sparql/{PEOPLE}", content=SELECT_NAME,
                      headers={"content-type": "application/sparql-query"})
    assert form.status_code == raw.status_code == 200
    assert form.json() == raw.json()
    assert form.json()["results"]["bindings"][0]["n"]["value"] == "Alice"


def test_ask_protocol(client):
    r = _sparql(client, PEOPLE, f"ASK {{ {ALICE} {KNOWS} {BOB} }}")
    assert r.json() == {"head": {}, "boolean": True}


def test_construct_quoted_triple_is_rdf12_triple_term(client):
    r = _sparql(client, PEOPLE, CONSTRUCT_CLAIM)
    assert r.status_code == 200, r.text
    assert r.headers["content-type"].startswith("application/n-triples")
    assert r.text == f"<{EX}claim1> <{EX}asserts> {QUOTED_RDF12} .\n"

    # The independent referee: an RDF 1.2 parser reads it as one triple whose
    # object is a triple term.
    (triple,) = list(ox.parse(r.text, format=ox.RdfFormat.N_TRIPLES))
    assert isinstance(triple.object, ox.Triple)
    assert triple.object.subject == ox.NamedNode(f"{EX}alice")


def test_construct_turtle_is_the_same_graph(client):
    r = client.get(f"/sparql/{PEOPLE}?query={quote(CONSTRUCT_CLAIM)}",
                   headers={"accept": "text/turtle"})
    assert r.headers["content-type"].startswith("text/turtle")
    # One triple. The old `<< s p o >>` body parsed as Turtle 1.2 is a reifier:
    # a blank node plus an rdf:reifies statement, a different graph.
    triples = list(ox.parse(r.text, format=ox.RdfFormat.TURTLE))
    assert len(triples) == 1 and isinstance(triples[0].object, ox.Triple)


def test_the_old_surface_is_still_available_and_is_what_parsers_reject(client):
    r = _sparql(client, PEOPLE, CONSTRUCT_CLAIM, quoted_triple_syntax="rdf-star")
    assert r.status_code == 200
    assert r.text == f"<{EX}claim1> <{EX}asserts> {QUOTED_STAR} .\n"
    # What relay used to serve by default: no RDF 1.2 N-Triples parser takes it.
    with pytest.raises(SyntaxError):
        list(ox.parse(r.text, format=ox.RdfFormat.N_TRIPLES))


def test_construct_rdf12_through_rest_and_describe(client):
    r = client.post("/api/query", json={"dataset": PEOPLE, "query": CONSTRUCT_CLAIM})
    assert r.json()["triples"] == [[f"<{EX}claim1>", f"<{EX}asserts>", QUOTED_RDF12]]

    import rete_service as svc

    doc = svc.describe_entity(PEOPLE, f"{EX}claim1")
    assert [f"<{EX}claim1>", f"<{EX}asserts>", QUOTED_RDF12] in doc["triples"]
    assert not any(t[2].startswith("<<<") for t in doc["triples"])


def test_subject_position_quoted_triple_is_refused_by_name(client):
    everything = "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }"
    r = _sparql(client, STAR_SUBJECT, everything)
    assert r.status_code == 406
    assert "OBJECT position only" in r.text and "quoted_triple_syntax=rdf-star" in r.text

    ok = _sparql(client, STAR_SUBJECT, everything, quoted_triple_syntax="rdf-star")
    assert ok.status_code == 200
    assert ok.text == f"{QUOTED_STAR} <{EX}source> <{EX}wiki> .\n"

    form = client.post(f"/sparql/{STAR_SUBJECT}",
                       data={"query": everything, "quoted_triple_syntax": "rdf-star"})
    assert form.text == ok.text

    rest = client.post("/api/query", json={"dataset": STAR_SUBJECT, "query": everything})
    assert rest.status_code == 400 and "rdf-star" in rest.json()["detail"]


def test_unknown_quoted_triple_syntax_is_a_client_error(client):
    r = _sparql(client, PEOPLE, CONSTRUCT_CLAIM, quoted_triple_syntax="turtle-star")
    assert r.status_code == 400 and "quoted_triple_syntax" in r.text


def test_graph_without_quoted_triples_is_unchanged(client):
    r = _sparql(client, PEOPLE, f"CONSTRUCT {{ ?s {KNOWS} ?o }} WHERE {{ ?s {KNOWS} ?o }}")
    assert r.text == f"{ALICE} {KNOWS} {BOB} .\n"
