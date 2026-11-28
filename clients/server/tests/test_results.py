"""Big answers stay out of the context: stored, summarised, paged, downloaded."""
from __future__ import annotations

import asyncio
import csv
import io
import json
import time

import httpx
import orjson
import pytest
import pyoxigraph

from conftest import PEOPLE, TOKEN
from helpers import tool_json
from live import ALL_TRIPLES, ALL_VALUES, BIG_ROWS, NUMBERS, live
from rete_server import results as results_mod

BEARER = {"Authorization": f"Bearer {TOKEN}"}
PEOPLE_ALL = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o"


@pytest.fixture(scope="module")
def server(tmp_path_factory):
    with live(tokens=[TOKEN], results_dir=tmp_path_factory.mktemp("results")) as base:
        yield base


def mcp(base):
    from fastmcp import Client
    from fastmcp.client.transports import StreamableHttpTransport

    return Client(StreamableHttpTransport(f"{base}/mcp", headers=BEARER))


def call(base, name, args):
    async def go():
        async with mcp(base) as c:
            return await c.call_tool(name, args)
    return asyncio.run(go())


def big_select(base):
    result = call(base, "sparql_query", {"dataset": NUMBERS, "query": ALL_VALUES})
    return result, tool_json(result)


# --------------------------------------------------------------------------- #
# The answer a client sees
# --------------------------------------------------------------------------- #

def test_small_answer_is_unchanged(server):
    result = call(server, "sparql_query", {"dataset": PEOPLE, "query": PEOPLE_ALL})
    doc = tool_json(result)
    assert len(result.content) == 1
    assert "result" not in doc and doc["truncated"] is False
    assert len(doc["table"]) == 4 and len(doc["results"]["bindings"]) == 4


def test_big_select_is_summarised_with_a_handle(server):
    result, doc = big_select(server)
    # The JSON text stays first; a resource_link follows it.
    assert result.content[0].type == "text" and result.content[1].type == "resource_link"
    assert str(result.content[1].uri) == doc["result"]["uri"] == f"rete://results/{doc['result']['id']}"
    assert doc["truncated"] is True and doc["total_rows"] == BIG_ROWS
    assert doc["head"] == {"vars": ["s", "v"]}
    assert len(doc["table"]) == doc["shown_rows"] == 20 and "results" not in doc
    assert doc["table"][0] == {"s": "http://ex.org/item0", "v": 0}
    handle = doc["result"]
    assert handle["rows"] == BIG_ROWS and handle["expires"].endswith("Z")
    assert set(handle["download"]) == {"json", "srj", "csv", "tsv"}
    # The links are absolute (the request's own Host) so `curl` can use them as given.
    assert handle["download"]["csv"].startswith(server)
    assert structured(result)["result"]["id"] == handle["id"]


def structured(result):
    return result.structured_content or result.data


def test_row_threshold_and_byte_budget():
    doc = {"kind": "select", "table": [{"a": 1}] * 10, "head": {"vars": ["a"]}}
    assert not results_mod.is_big(doc, 200, 10_000)
    assert results_mod.is_big(doc, 5, 10_000)          # too many rows
    assert results_mod.is_big(doc, 200, 50)            # too many bytes
    assert not results_mod.is_big({"kind": "ask", "boolean": True}, 0, 0)


# --------------------------------------------------------------------------- #
# Paging: the tool and the resource
# --------------------------------------------------------------------------- #

def test_result_rows_pages_through_the_answer(server):
    async def go():
        async with mcp(server) as c:
            doc = tool_json(await c.call_tool("sparql_query", {"dataset": NUMBERS, "query": ALL_VALUES}))
            rid = doc["result"]["id"]
            seen, offset, pages = [], 0, 0
            while offset is not None:
                page = tool_json(await c.call_tool("result_rows", {"result": rid, "offset": offset, "limit": 200}))
                assert page["total_rows"] == BIG_ROWS and page["columns"] == ["s", "v"]
                seen += page["rows"]
                offset, pages = page["next_offset"], pages + 1
            # The uri works as the reference too.
            again = tool_json(await c.call_tool("result_rows", {"result": doc["result"]["uri"], "limit": 3}))
            tools = {t.name: t for t in await c.list_tools()}
            return seen, pages, again, tools["result_rows"]

    seen, pages, again, tool = asyncio.run(go())
    assert pages == 3 and [r["v"] for r in seen] == list(range(BIG_ROWS))
    assert len(again["rows"]) == 3 and again["next_offset"] == 3
    assert tool.annotations.read_only_hint is True


def test_resource_template_reads_a_page(server):
    async def go():
        async with mcp(server) as c:
            doc = tool_json(await c.call_tool("sparql_query", {"dataset": NUMBERS, "query": ALL_VALUES}))
            uri = doc["result"]["uri"]
            first = json.loads((await c.read_resource(uri))[0].text)
            tail = json.loads((await c.read_resource(f"{uri}?offset=450&limit=100"))[0].text)
            templates = [t.uri_template for t in await c.list_resource_templates()]
            return first, tail, templates

    first, tail, templates = asyncio.run(go())
    assert first["returned"] == 200 and first["next_offset"] == 200 and first["offset"] == 0
    assert tail["offset"] == 450 and tail["returned"] == 50 and tail["next_offset"] is None
    assert tail["rows"][-1]["v"] == BIG_ROWS - 1
    assert any(t.startswith("rete://results/") for t in templates)


def test_unknown_result_is_a_clear_error(server):
    from fastmcp.exceptions import ToolError

    with pytest.raises(ToolError, match="unknown or has expired"):
        call(server, "result_rows", {"result": "rete://results/" + "x" * 32})
    with pytest.raises(ToolError, match="unknown or has expired"):
        call(server, "result_rows", {"result": "../../etc/passwd"})


# --------------------------------------------------------------------------- #
# CONSTRUCT
# --------------------------------------------------------------------------- #

def test_big_construct_is_stored_and_downloads_as_ntriples(server):
    result = call(server, "sparql_query", {"dataset": NUMBERS, "query": ALL_TRIPLES})
    doc = tool_json(result)
    assert doc["kind"] == "construct" and doc["truncated"] and doc["total_rows"] == BIG_ROWS
    assert len(doc["triples"]) == 20 and set(doc["result"]["download"]) == {"json", "nt"}
    page = tool_json(call(server, "result_rows", {"result": doc["result"]["id"], "offset": 490, "limit": 50}))
    assert len(page["triples"]) == 10 and page["next_offset"] is None
    nt = httpx.get(f"{server}{doc['result']['download']['nt'].removeprefix(server)}", headers=BEARER)
    assert nt.status_code == 200 and nt.headers["content-type"].startswith("application/n-triples")
    parsed = list(pyoxigraph.parse(nt.content, format=pyoxigraph.RdfFormat.N_TRIPLES))
    assert len(parsed) == BIG_ROWS


# --------------------------------------------------------------------------- #
# REST download
# --------------------------------------------------------------------------- #

def test_rest_downloads_every_format(server):
    _, doc = big_select(server)
    rid = doc["result"]["id"]

    def get(fmt):
        return httpx.get(f"{server}/api/results/{rid}", params={"format": fmt}, headers=BEARER)

    js = get("json")
    assert js.status_code == 200 and js.headers["content-type"].startswith("application/json")
    body = js.json()
    assert body["head"] == {"vars": ["s", "v"]} and len(body["results"]["bindings"]) == BIG_ROWS
    assert body["total_rows"] == BIG_ROWS and body["dataset"] == NUMBERS and "stats" in body

    srj = get("srj")
    assert srj.headers["content-type"].startswith("application/sparql-results+json")
    assert set(srj.json()) == {"head", "results"}
    assert srj.json()["results"]["bindings"][1]["v"] == {
        "type": "literal", "value": "1", "datatype": "http://www.w3.org/2001/XMLSchema#integer"}

    rows = list(csv.reader(io.StringIO(get("csv").text)))
    assert rows[0] == ["s", "v"] and len(rows) == BIG_ROWS + 1 and rows[1] == ["http://ex.org/item0", "0"]

    tsv = get("tsv").text.splitlines()
    assert tsv[0] == "?s\t?v" and len(tsv) == BIG_ROWS + 1
    assert tsv[1] == '<http://ex.org/item0>\t"0"^^<http://www.w3.org/2001/XMLSchema#integer>'

    assert get("nt").status_code == 400          # N-Triples is for CONSTRUCT
    assert get("xml").status_code == 400
    assert get("csv").headers["content-disposition"].startswith("attachment")


def test_rest_needs_the_token_and_a_real_id(server):
    _, doc = big_select(server)
    url = f"{server}/api/results/{doc['result']['id']}"
    assert httpx.get(url).status_code == 401
    assert httpx.get(url, headers={"Authorization": "Bearer wrong"}).status_code == 401
    assert httpx.get(url, headers=BEARER).status_code == 200
    assert httpx.get(f"{server}/api/results/does-not-exist-1234567890", headers=BEARER).status_code == 404
    assert httpx.get(f"{server}/api/results/short", headers=BEARER).status_code == 404


# --------------------------------------------------------------------------- #
# The store: expiry and the size cap
# --------------------------------------------------------------------------- #

def _rows(n):
    return [[{"x": i}, {"x": {"type": "literal", "value": str(i)}}] for i in range(n)]


META = {"dataset": "d", "kind": "select", "vars": ["x"], "elapsed_seconds": 0, "stats": {}, "warnings": []}


def test_results_expire(tmp_path):
    store = results_mod.ResultStore(tmp_path, 1 << 20, ttl_s=1)
    handle = store.put(META, _rows(5))
    assert store.page(handle["id"])["total_rows"] == 5
    time.sleep(1.3)
    with pytest.raises(results_mod.ResultGone):
        store.page(handle["id"])
    assert not list(tmp_path.glob("*.jsonl"))          # deleted on the read that found it expired
    # ... and swept by the next write even if nobody asks for it again.
    old = store.put(META, _rows(5))
    time.sleep(1.3)
    store.put(META, _rows(5))
    assert not (tmp_path / f"{old['id']}.jsonl").exists()


def test_store_evicts_the_oldest_past_its_cap(tmp_path):
    one = len(b"".join(orjson.dumps(r) + b"\n" for r in _rows(50)))
    store = results_mod.ResultStore(tmp_path, max_bytes=int(one * 2.6), ttl_s=3600)
    ids = []
    for _ in range(5):
        ids.append(store.put(META, _rows(50))["id"])
        time.sleep(0.02)
    alive = [i for i in ids if (tmp_path / f"{i}.jsonl").exists()]
    assert alive == ids[-2:]                           # the newest two fit, the older three are gone
    assert sum(p.stat().st_size for p in tmp_path.glob("*.jsonl")) <= store.max_bytes
    with pytest.raises(results_mod.ResultGone):
        store.head(ids[0])


def test_ids_cannot_escape_the_directory(tmp_path):
    store = results_mod.ResultStore(tmp_path, 1 << 20, 60)
    for bad in ("../x", "a/b", "", "x" * 3, "..%2f..%2fetc" + "a" * 20):
        with pytest.raises(results_mod.ResultGone):
            store.head(bad)
    assert len(results_mod.ResultStore.parse_ref("rete://results/abc?offset=2")) == 3
