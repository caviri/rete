"""Authoring: build a .rete, query it at once, download it by range."""
from __future__ import annotations

import shutil

import pytest

from conftest import serve

TTL = """@prefix ex: <http://ex.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:a rdfs:label "A" ; ex:knows ex:b .
ex:b rdfs:label "B" .
"""


def test_build_query_and_download(client):
    r = client.post("/api/author/build", json={"rdf": TTL, "card": {"title": "Tiny"},
                                                "examples": [{"title": "All", "sparql": "SELECT * WHERE { ?s ?p ?o }"}]})
    assert r.status_code == 200, r.text
    built = r.json()
    key, url = built["dataset"], built["url"]
    assert key.startswith("generated/") and url == f"/{key}"

    listed = {d["key"]: d for d in client.get("/api/datasets").json()}
    assert listed[key]["label"] == "Tiny"
    q = client.post("/api/query", json={"dataset": key, "query": "SELECT ?l WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l }"})
    assert sorted(row["l"] for row in q.json()["table"]) == ["A", "B"]

    whole = client.get(url)
    assert whole.status_code == 200 and len(whole.content) == built["bytes"]
    part = client.get(url, headers={"Range": "bytes=0-7"})
    assert part.status_code == 206 and part.content == whole.content[:8]
    assert client.head(url).headers["content-length"] == str(built["bytes"])


def test_generated_path_traversal_is_404(client):
    assert client.get("/generated/..%2Fpeople.rete").status_code == 404
    assert client.get("/generated/.hidden.rete").status_code == 404


def test_authoring_can_be_turned_off():
    with serve(author=False) as c:
        assert c.post("/api/author/build", json={"rdf": TTL}).status_code in (404, 405)
        assert c.get("/generated/x.rete").status_code == 404


def test_causal_diagram_builds_a_queryable_graph(client):
    r = client.post("/api/author/causal", json={"claims": [
        {"cause": "Rain", "effect": "Wet streets", "relation": "causes", "speaker": "Ana"},
        {"cause": "Umbrellas", "effect": "Wet clothes", "relation": "prevents"}]})
    assert r.status_code == 200, r.text
    doc = r.json()
    assert doc["claims"] == 2 and "flowchart LR" in doc["mermaid"]
    rows = client.post("/api/query", json={"dataset": doc["dataset"], "query":
                       "SELECT ?c WHERE { ?c a <https://w3id.org/rete/causal-conv#Claim> }"}).json()["table"]
    assert len(rows) == 2
    if not shutil.which("dot"):
        pytest.skip("graphviz not installed here; the image carries it")
    assert doc["svg_data_uri"].startswith("data:image/svg+xml;base64,")
