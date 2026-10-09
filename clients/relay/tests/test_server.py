"""The relay as it runs in production: `uvicorn app:app` (the Dockerfile's
CMD) on a real port, answering /health, the REST and SPARQL planes, and MCP
over streamable HTTP at /mcp/."""
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

from conftest import CONTAINS_ON_IRI, ENV, PEOPLE, RELAY
from helpers import tool_json


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def server():
    port = _free_port()
    proc = subprocess.Popen(
        [sys.executable, "-m", "uvicorn", "app:app", "--host", "127.0.0.1",
         "--port", str(port), "--workers", "1"],
        cwd=RELAY, env={**os.environ, **ENV},
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.time() + 60
        while True:
            if proc.poll() is not None:
                pytest.fail(f"uvicorn exited {proc.returncode}:\n{proc.stdout.read()}")
            try:
                if httpx.get(f"{base}/health", timeout=2).status_code == 200:
                    break
            except httpx.TransportError:
                pass
            if time.time() > deadline:
                pytest.fail("relay did not answer /health within 60 s")
            time.sleep(0.25)
        yield base
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()


def test_health_and_catalog(server):
    assert httpx.get(f"{server}/health").json()["ok"] is True
    keys = {d["key"] for d in httpx.get(f"{server}/api/datasets").json()}
    assert PEOPLE in keys


def test_rest_and_protocol_answer(server):
    r = httpx.post(f"{server}/api/query", json={"dataset": PEOPLE, "query": CONTAINS_ON_IRI})
    assert r.status_code == 200 and r.json()["warnings"][0]["function"] == "CONTAINS"
    r = httpx.get(f"{server}/sparql/{PEOPLE}?query={quote(CONTAINS_ON_IRI)}",
                  headers={"Origin": "https://example.org"})
    assert r.status_code == 200 and "CONTAINS" in r.headers["x-rete-warnings"]
    # A browser on another origin may read the header.
    assert "x-rete-warnings" in r.headers["access-control-expose-headers"].lower()


def test_mcp_over_streamable_http(server):
    from fastmcp import Client

    async def go():
        async with Client(f"{server}/mcp/") as c:
            names = {t.name for t in await c.list_tools()}
            doc = tool_json(await c.call_tool(
                "sparql_query", {"dataset": PEOPLE, "query": CONTAINS_ON_IRI}))
            return names, doc

    names, doc = asyncio.run(go())
    assert {"sparql_query", "list_datasets", "describe_entity"} <= names
    assert doc["table"] == [] and doc["warnings"][0]["argKind"] == "iri"
