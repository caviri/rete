"""The server as it runs in production: `rete-server` (the image's
entrypoint) on a real port, configured by flags and environment, answering
/health, REST, the SPARQL Protocol and MCP over streamable HTTP, behind a
token."""
from __future__ import annotations

import asyncio
import os
import socket
import subprocess
import sys
import time
from urllib.parse import quote

import httpx
import pytest

from conftest import CATALOG, CONTAINS_ON_IRI, DATA, PEOPLE, ROOT, TOKEN
from helpers import tool_json

BEARER = {"Authorization": f"Bearer {TOKEN}"}


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def server():
    port = _free_port()
    token_file = ROOT / "tokens.txt"
    token_file.write_text(f"# rotated weekly\n{TOKEN}\n", encoding="utf-8")
    env = {**os.environ, "RETE_TOKEN_FILE": str(token_file), "RETE_DATA_DIR": str(DATA),
           "RETE_GENERATED_DIR": str(ROOT / "generated-server")}
    proc = subprocess.Popen(
        [sys.executable, "-m", "rete_server", "--host", "127.0.0.1", "--port", str(port),
         "--catalog", str(CATALOG), "--warm", "people", "--log-level", "warning"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + 60
        while True:
            if proc.poll() is not None:
                pytest.fail(f"rete-server exited {proc.returncode}:\n{proc.stdout.read()}")
            try:
                if httpx.get(f"{base}/health", timeout=2).status_code == 200:
                    break
            except httpx.TransportError:
                pass
            if time.time() > deadline:
                pytest.fail("rete-server did not answer /health within 60 s")
            time.sleep(0.25)
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()


def test_health_open_catalog_locked(server):
    assert httpx.get(f"{server}/health").json()["ok"] is True
    assert httpx.get(f"{server}/api/datasets").status_code == 401
    keys = {d["key"] for d in httpx.get(f"{server}/api/datasets", headers=BEARER).json()}
    assert {"people", PEOPLE} <= keys


def test_rest_and_protocol_answer(server):
    r = httpx.post(f"{server}/api/query", headers=BEARER,
                   json={"dataset": PEOPLE, "query": CONTAINS_ON_IRI})
    assert r.status_code == 200 and r.json()["warnings"][0]["function"] == "CONTAINS"
    r = httpx.get(f"{server}/sparql/{PEOPLE}?query={quote(CONTAINS_ON_IRI)}",
                  headers={**BEARER, "Origin": "https://example.org"})
    assert r.status_code == 200 and "CONTAINS" in r.headers["x-rete-warnings"]
    assert "x-rete-warnings" in r.headers["access-control-expose-headers"].lower()


def test_post_protocol_query_does_not_block_other_requests(server):
    # The relay ran POST /sparql queries on the event loop; here they run in
    # the thread pool, so /health answers while queries are in flight.
    async def go():
        async with httpx.AsyncClient(base_url=server, headers=BEARER, timeout=30) as c:
            queries = [c.post(f"/sparql/{PEOPLE}", data={"query": "SELECT * WHERE { ?s ?p ?o }"})
                       for _ in range(8)]
            results = await asyncio.gather(*queries, c.get("/health"))
            return [r.status_code for r in results]

    assert set(asyncio.run(go())) == {200}


@pytest.mark.parametrize("path", ["/mcp", "/mcp/"])
def test_mcp_over_streamable_http(server, path):
    from fastmcp import Client
    from fastmcp.client.transports import StreamableHttpTransport

    async def go():
        async with Client(StreamableHttpTransport(f"{server}{path}", headers=BEARER)) as c:
            names = {t.name for t in await c.list_tools()}
            doc = tool_json(await c.call_tool(
                "sparql_query", {"dataset": PEOPLE, "query": CONTAINS_ON_IRI}))
            return names, doc

    names, doc = asyncio.run(go())
    assert {"sparql_query", "list_datasets", "describe_entity", "causal_diagram"} <= names
    assert doc["table"] == [] and doc["warnings"][0]["argKind"] == "iri"


def test_mcp_without_the_token_is_refused(server):
    r = httpx.post(f"{server}/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                   headers={"accept": "application/json, text/event-stream"})
    assert r.status_code == 401


def test_warmed_dataset_is_already_open(server):
    deadline = time.time() + 10
    while time.time() < deadline:
        handles = httpx.get(f"{server}/api/status", headers=BEARER).json()["open_handles"]
        if any(h["source"].endswith("people-copy.rete") for h in handles):
            return
        time.sleep(0.2)
    pytest.fail("--warm people did not open the dataset")
