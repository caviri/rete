"""The token gate: every route but /health, header or (opt-in) query token."""
from __future__ import annotations

import asyncio
from urllib.parse import quote

import pytest

from conftest import PEOPLE, TOKEN, serve

BEARER = {"Authorization": f"Bearer {TOKEN}"}
ASK = quote("ASK { ?s ?p ?o }")


@pytest.mark.parametrize("path", ["/", "/api/datasets", f"/sparql/{PEOPLE}?query={ASK}",
                                  "/docs", "/openapi.json", "/api/status", "/generated/x.rete"])
def test_everything_but_health_needs_the_token(locked, path):
    r = locked.get(path)
    assert r.status_code == 401, (path, r.status_code)
    assert r.headers["www-authenticate"].startswith("Bearer")
    assert locked.get(path, headers=BEARER).status_code != 401


def test_health_stays_open(locked):
    r = locked.get("/health")
    assert r.status_code == 200 and r.json()["ok"] is True


def test_mcp_needs_the_token(locked):
    r = locked.post("/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                    headers={"accept": "application/json, text/event-stream"})
    assert r.status_code == 401


def test_wrong_token_and_wrong_scheme_are_refused(locked):
    assert locked.get("/api/datasets", headers={"Authorization": "Bearer nope"}).status_code == 401
    assert locked.get("/api/datasets", headers={"Authorization": f"Basic {TOKEN}"}).status_code == 401


def test_query_token_is_off_by_default(locked):
    assert locked.get(f"/api/datasets?token={TOKEN}").status_code == 401


def test_query_token_when_enabled_and_stripped_before_the_app():
    with serve(tokens=[TOKEN], query_token=True) as c:
        r = c.get(f"/sparql/{PEOPLE}?query={ASK}&token={TOKEN}")
        assert r.status_code == 200 and r.json()["boolean"] is True
        # The app never sees it: the service description echoes the URL it got.
        r = c.get(f"/sparql/{PEOPLE}?token={TOKEN}")
        assert r.status_code == 200 and TOKEN not in r.text


def test_several_tokens_any_one_opens():
    with serve(tokens=["old-token", TOKEN]) as c:
        for t in ("old-token", TOKEN):
            assert c.get("/api/datasets", headers={"Authorization": f"Bearer {t}"}).status_code == 200


def test_cors_preflight_and_401_carry_cors_headers(locked):
    pre = locked.options("/api/query", headers={
        "Origin": "https://example.org", "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "authorization,content-type"})
    assert pre.status_code == 200
    assert "authorization" in pre.headers["access-control-allow-headers"].lower()
    denied = locked.get("/api/datasets", headers={"Origin": "https://example.org"})
    assert denied.status_code == 401 and denied.headers["access-control-allow-origin"] == "*"


def test_mcp_client_with_bearer_header(locked):
    """The FastMCP client, as Claude Code would connect with --header."""
    from fastmcp import Client
    from fastmcp.client.transports import StreamableHttpTransport

    # TestClient is synchronous; drive the ASGI app through httpx directly.
    import httpx

    app = locked.app

    async def go():
        transport = StreamableHttpTransport(
            "http://testserver/mcp", headers=BEARER,
            httpx_client_factory=lambda **kw: httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), **kw))
        async with Client(transport) as c:
            return {t.name for t in await c.list_tools()}

    names = asyncio.run(go())
    assert {"sparql_query", "list_datasets", "build_rete"} <= names
