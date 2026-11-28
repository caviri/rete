"""Catalog sources: a file, a URL (ETag refresh, fallback), the data folder."""
from __future__ import annotations

import json

import httpx
import rete_graph as rete

from conftest import DATA, PEOPLE, PEOPLE_NT, ROOT, settings
from rete_server import catalog as catalog_mod
from rete_server.catalog import Catalog
from rete_server.settings import DEFAULT_CATALOG_URL

# A generated-files folder nothing writes to, so other tests' builds stay out.
EMPTY = ROOT / "no-generated"


def test_file_catalog_and_local_folder_merge(client):
    keys = {d["key"] for d in client.get("/api/datasets").json()}
    assert {"people", PEOPLE, "local/star-subject.rete"} <= keys
    # Server-side paths never leave the server.
    doc = client.get("/api/datasets/people").json()
    assert "local_path" not in doc and "card" in doc  # these test files carry no card


def test_relative_url_in_a_file_catalog_is_a_file_beside_it(client):
    r = client.post("/api/query", json={"dataset": "people", "query": "ASK { ?s ?p ?o }"})
    assert r.status_code == 200 and r.json()["boolean"] is True


def test_catalog_examples_are_served(client):
    examples = client.get("/api/datasets/people/examples").json()
    assert examples and examples[0]["sparql"].startswith("SELECT")


def test_local_cards_fill_labels():
    built = rete.Builder().add(PEOPLE_NT, format="nt").card(title="Carded people").run()
    path = DATA / "sub" / "carded.rete"
    path.parent.mkdir(exist_ok=True)
    path.write_bytes(built)
    try:
        cat = Catalog(settings())
        cat.load()
        assert cat.get("local/sub/carded.rete")["label"] == "carded"
        cat.read_local_cards()
        assert cat.get("local/sub/carded.rete")["label"] == "Carded people"
    finally:
        path.unlink()


def test_hidden_files_are_not_served():
    hidden = DATA / ".cache"
    hidden.mkdir(exist_ok=True)
    (hidden / "x.rete").write_bytes((DATA / "people.rete").read_bytes())
    cat = Catalog(settings())
    cat.load()
    assert not any(k.startswith("local/.cache") for k in cat.keys())


def _url_catalog(handler, source="https://catalog.example/catalog.json"):
    http = httpx.Client(transport=httpx.MockTransport(handler))
    return Catalog(settings(catalogs=[source], data_dir=None, generated_dir=EMPTY), http=http)


def test_url_catalog_resolves_relative_urls_and_refreshes_by_etag():
    calls = []

    def handler(request):
        calls.append(request.headers.get("if-none-match"))
        if request.headers.get("if-none-match") == '"v1"':
            return httpx.Response(304)
        return httpx.Response(200, headers={"etag": '"v1"'}, json={"datasets": [
            {"key": "a", "url": "a/a.rete"}, {"key": "b", "url": "https://elsewhere.example/b.rete"}]})

    cat = _url_catalog(handler)
    cat.load()
    assert cat.get("a")["url"] == "https://catalog.example/a/a.rete"
    assert cat.get("b")["url"] == "https://elsewhere.example/b.rete"
    cat.load()
    assert calls == [None, '"v1"'] and cat.get("a") is not None


def test_failed_refresh_keeps_the_last_good_copy():
    state = {"up": True}

    def handler(request):
        if state["up"]:
            return httpx.Response(200, json={"datasets": [{"key": "a", "url": "https://x.example/a.rete"}]})
        return httpx.Response(503)

    cat = _url_catalog(handler)
    cat.load()
    state["up"] = False
    cat.load()
    assert cat.get("a") is not None
    assert "stale" in cat.status()["sources"]["https://catalog.example/catalog.json"]


def test_default_catalog_falls_back_to_the_bundled_copy():
    cat = _url_catalog(lambda request: httpx.Response(404), source=DEFAULT_CATALOG_URL)
    cat.load()
    bundled = json.loads(catalog_mod.BUNDLED_CATALOG.read_text(encoding="utf-8"))["datasets"]
    assert len(cat.keys()) == len(bundled) > 0
    assert "bundled" in cat.status()["sources"][DEFAULT_CATALOG_URL]


def test_another_unreadable_url_is_not_replaced_by_the_bundled_copy():
    cat = _url_catalog(lambda request: httpx.Response(404))
    cat.load()
    assert cat.keys() == []


def test_new_local_file_is_found_without_a_restart(client):
    path = DATA / "late.rete"
    path.write_bytes((DATA / "people.rete").read_bytes())
    try:
        from rete_server import state

        state.catalog._last_local_scan = 0  # past the rescan rate limit
        r = client.post("/api/query", json={"dataset": "local/late.rete", "query": "ASK { ?s ?p ?o }"})
        assert r.status_code == 200, r.text
    finally:
        path.unlink()


def test_no_catalog_at_all(tmp_path):
    cat = Catalog(settings(catalogs=[], data_dir=tmp_path, generated_dir=EMPTY))
    cat.load()
    assert cat.keys() == [] and cat.status()["datasets"] == 0
