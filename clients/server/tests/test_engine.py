"""Handles, guards and the url= policy."""
from __future__ import annotations

import threading
import time
from urllib.parse import quote

import pytest

from conftest import ALICE, KNOWS, PEOPLE, serve
from rete_server import state
from rete_server.engine import RequestError, check_public_url


@pytest.mark.parametrize("url", [
    "http://127.0.0.1/x.rete", "http://localhost/x.rete", "http://10.0.0.5/x.rete",
    "http://169.254.169.254/latest/meta-data", "http://[::1]/x.rete", "http://192.168.1.2/x.rete",
])
def test_private_addresses_are_refused(url):
    with pytest.raises(RequestError, match="non-public"):
        check_public_url(url)


def test_non_http_urls_are_refused():
    with pytest.raises(RequestError):
        check_public_url("file:///etc/passwd")


def test_url_queries_can_be_turned_off():
    with serve(allow_urls=False) as c:
        r = c.post("/api/query", json={"url": "https://data.example/x.rete", "query": "ASK {}"})
        assert r.status_code == 400 and "catalog" in r.json()["detail"]


def test_private_url_is_a_400_not_a_fetch(client):
    r = client.get(f"/sparql/http://127.0.0.1:9/x.rete?query={quote('ASK { ?s ?p ?o }')}")
    assert r.status_code == 400 and "non-public" in r.text


def test_unknown_dataset_is_404_everywhere(client):
    assert client.get("/api/datasets/nope/card").status_code == 404
    assert client.get(f"/sparql/nope?query={quote('ASK {}')}").status_code == 404


def test_bad_sparql_is_400(client):
    r = client.post("/api/query", json={"dataset": PEOPLE, "query": "SELEKT nothing"})
    assert r.status_code == 400


def test_row_cap_truncates_and_says_so(client):
    r = client.post("/api/query", json={"dataset": PEOPLE, "query": "SELECT * WHERE { ?s ?p ?o }",
                                        "limit": 2})
    doc = r.json()
    assert len(doc["table"]) == 2 and doc["truncated"] is True


def test_describe_entity_refuses_an_injected_iri(client):
    with pytest.raises(RequestError):
        state.service.describe_entity(PEOPLE, "http://ex.org/a> } ; DROP ALL ; <x")


def test_concurrent_first_queries_open_the_dataset_once(client):
    opened = []
    real_open = state.service._open

    def counting_open(source):
        opened.append(source)
        time.sleep(0.2)  # a slow open, so the other threads arrive while it runs
        return real_open(source)

    state.service._open = counting_open
    errors = []

    def query():
        try:
            state.service.run_query("local/star-subject.rete", None, "ASK { ?s ?p ?o }")
        except Exception as e:  # pragma: no cover - reported below
            errors.append(e)

    threads = [threading.Thread(target=query) for _ in range(6)]
    [t.start() for t in threads]
    [t.join() for t in threads]
    assert not errors and opened.count(state.service.resolve("local/star-subject.rete")[0]) == 1


def test_a_busy_dataset_answers_503_instead_of_queueing_forever():
    with serve(query_timeout_s=0.2) as c:
        handle = state.service.handle(state.service.resolve(PEOPLE)[0])
        with handle.lock:  # someone else's long query
            r = c.post("/api/query", json={"dataset": PEOPLE, "query": "ASK { ?s ?p ?o }"})
        assert r.status_code == 503 and "busy" in r.json()["detail"]


def test_status_lists_open_handles(client):
    client.post("/api/query", json={"dataset": PEOPLE, "query": f"ASK {{ {ALICE} {KNOWS} ?o }}"})
    doc = client.get("/api/status").json()
    assert doc["catalog"]["datasets"] >= 3
    assert any(h["source"].endswith("people.rete") for h in doc["open_handles"])


def test_home_page_is_a_shell_that_fetches_its_data(client):
    page = client.get("/")
    assert page.status_code == 200 and page.headers["content-type"].startswith("text/html")
    assert "Connect an agent" in page.text and 'id="drop"' in page.text
    assert "claude mcp add" in client.get("/home/home.js").text
    assert "local/people.rete" not in page.text  # no data in the shell: the page asks /api for it
