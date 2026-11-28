"""Long queries as MCP tasks (io.modelcontextprotocol/tasks, protocol 2026-07-28).

A real server on a real port; the engine is slowed with ``# slow <seconds>`` in
the query (see live.py). Clients: the FastMCP client (declares the extension on
every request), the same client pinned to the old handshake, and raw HTTP for
a modern client that does not declare it.
"""
from __future__ import annotations

import asyncio
import time

import httpx
import pytest
from fastmcp import Client
from fastmcp.client.transports import StreamableHttpTransport
from fastmcp.exceptions import ToolError
from fastmcp_tasks import call_tool_task

from conftest import PEOPLE, TOKEN
from helpers import tool_json
from live import ALL_VALUES, BIG_ROWS, NUMBERS, live, make_slow

BEARER = {"Authorization": f"Bearer {TOKEN}"}
TASKS = "io.modelcontextprotocol/tasks"
VERSION = "2026-07-28"
ALL = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o"


def slow(seconds: float) -> dict:
    return {"dataset": PEOPLE, "query": f"{ALL} # slow {seconds}"}


def client(base, **kw):
    return Client(StreamableHttpTransport(f"{base}/mcp", headers=BEARER), **kw)


def meta(declare: bool) -> dict:
    caps = {"extensions": {TASKS: {}}} if declare else {}
    return {"io.modelcontextprotocol/protocolVersion": VERSION,
            "io.modelcontextprotocol/clientCapabilities": caps}


def rpc(base, method, params, declare=True, token=True):
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
               "MCP-Protocol-Version": VERSION, "Mcp-Method": method}
    if method == "tools/call":
        headers["Mcp-Name"] = params["name"]
    if token:
        headers.update(BEARER)
    return httpx.post(f"{base}/mcp", headers=headers, timeout=30, json={
        "jsonrpc": "2.0", "id": 1, "method": method, "params": {**params, "_meta": meta(declare)}})


@pytest.fixture(scope="module")
def server(tmp_path_factory):
    with live(tokens=[TOKEN], results_dir=tmp_path_factory.mktemp("results"),
              query_timeout_s=60, task_timeout_s=60) as base:
        make_slow(PEOPLE)
        yield base


# --------------------------------------------------------------------------- #
# Negotiation
# --------------------------------------------------------------------------- #

def test_server_discover_advertises_the_tasks_extension(server):
    r = rpc(server, "server/discover", {})
    assert r.status_code == 200
    assert TASKS in r.json()["result"]["capabilities"]["extensions"]
    assert VERSION in r.json()["result"]["supportedVersions"]
    # The token gate is in front of discovery too.
    assert rpc(server, "server/discover", {}, token=False).status_code == 401


def test_a_modern_client_that_did_not_declare_tasks_gets_the_answer(server):
    r = rpc(server, "tools/call", {"name": "sparql_query", "arguments": slow(0)}, declare=False)
    body = r.json()["result"]
    assert body.get("resultType") != "task" and "taskId" not in body
    assert body["isError"] is False and '"kind":"select"' in body["content"][0]["text"]
    # The same call from a declaring client is a task.
    created = rpc(server, "tools/call", {"name": "sparql_query", "arguments": slow(0)}).json()["result"]
    assert created["resultType"] == "task" and created["status"] == "working"
    assert created["pollIntervalMs"] > 0 and created["ttlMs"] > 0


def test_an_old_handshake_client_gets_the_answer(server):
    async def go():
        async with client(server, mode="legacy") as c:
            return tool_json(await c.call_tool("sparql_query", slow(0)))

    doc = asyncio.run(go())
    assert doc["kind"] == "select" and len(doc["table"]) == 4


def test_only_the_long_tools_are_task_capable():
    from rete_server.mcp import build_mcp

    async def modes():
        mcp = build_mcp(author=True)
        names = ("sparql_query", "validate_shacl", "build_rete", "result_rows", "list_datasets", "dataset_card")
        return {n: (await mcp.get_tool(n)).task_config.mode for n in names}

    found = asyncio.run(modes())
    for name in ("sparql_query", "validate_shacl", "build_rete"):
        assert found[name] == "optional", name
    for name in ("result_rows", "list_datasets", "dataset_card"):
        assert found[name] == "forbidden", name


# --------------------------------------------------------------------------- #
# The life of a task
# --------------------------------------------------------------------------- #

def test_a_task_runs_to_completed_with_a_status_message(server):
    async def go():
        async with client(server) as c:
            task = await call_tool_task(c, "sparql_query", slow(2))
            created = task.create_result
            await asyncio.sleep(1.0)
            mid = await task.status()
            final = await task.wait(timeout=30)
            result = await task.result()
            return created, mid, final, result

    created, mid, final, result = asyncio.run(go())
    assert created.status == "working"
    assert mid.status == "working" and PEOPLE in mid.status_message and "elapsed" in mid.status_message
    assert final.status == "completed"
    doc = tool_json(result)
    assert doc["kind"] == "select" and len(doc["table"]) == 4 and doc["elapsed_seconds"] >= 2


def test_a_bad_query_ends_as_a_completed_task_with_an_error_result(server):
    """A query that fails is a tool error, which SEP-2663 reports as `completed`
    with `isError`; `failed` is for the task machinery itself."""
    async def go():
        async with client(server) as c:
            task = await call_tool_task(c, "sparql_query", {"dataset": PEOPLE, "query": "SELEC nope"},
                                        raise_on_error=False)
            final = await task.wait(timeout=30)
            unknown = await call_tool_task(c, "sparql_query", {"dataset": "no-such-dataset", "query": ALL},
                                           raise_on_error=False)
            return final, await unknown.wait(timeout=30)

    final, unknown = asyncio.run(go())
    assert final.status == "completed" and final.result["isError"] is True
    assert "parse error" in final.result["content"][0]["text"]
    assert unknown.status == "completed" and unknown.result["isError"] is True


def test_cancel_discards_the_result_and_the_server_stays_usable(server):
    async def go():
        async with client(server) as c:
            task = await call_tool_task(c, "sparql_query", slow(3))
            await asyncio.sleep(0.5)
            await task.cancel()
            right_after = (await task.status()).status
            # The engine cannot be interrupted: it finishes its query. The task stays cancelled.
            await asyncio.sleep(3.5)
            later = await task.status()
            # ... and the lane is free again.
            next_task = await call_tool_task(c, "sparql_query", slow(0))
            done = await next_task.wait(timeout=30)
            return right_after, later, done

    right_after, later, done = asyncio.run(go())
    assert right_after == "cancelled" and later.status == "cancelled" and later.result is None
    assert done.status == "completed"


def test_a_big_task_result_goes_through_the_store(server):
    async def go():
        async with client(server) as c:
            task = await call_tool_task(c, "sparql_query", {"dataset": NUMBERS, "query": ALL_VALUES})
            result = await task.result()
            doc = tool_json(result)
            page = tool_json(await c.call_tool("result_rows", {"result": doc["result"]["id"], "offset": 495}))
            return result, doc, page

    result, doc = asyncio.run(go())[:2]
    assert doc["truncated"] and doc["total_rows"] == BIG_ROWS and len(doc["table"]) == 20
    assert [c.type for c in result.content] == ["text", "resource_link"]


def test_a_tasks_get_without_the_extension_is_refused(server):
    created = rpc(server, "tools/call", {"name": "sparql_query", "arguments": slow(0)}).json()["result"]
    r = rpc(server, "tasks/get", {"taskId": created["taskId"]}, declare=False)
    assert r.json()["error"]["code"] == -32021


# --------------------------------------------------------------------------- #
# The per-dataset lock
# --------------------------------------------------------------------------- #

def test_a_long_task_does_not_make_quick_calls_busy():
    """With one handle per dataset a long query holds its lock and a quick call
    on the same dataset waits for it, then comes back Busy. Tasks run on their
    own handle, so the quick call is answered at once."""
    with live(tokens=[TOKEN], query_timeout_s=1, task_timeout_s=30) as base:
        make_slow(PEOPLE)

        async def go():
            async with client(base) as c, client(base, mode="legacy") as quick:
                # Before: two ordinary calls, the second behind a slow first one.
                first = asyncio.create_task(quick.call_tool("sparql_query", slow(2.5), raise_on_error=False))
                await asyncio.sleep(0.3)
                started = time.monotonic()
                with pytest.raises(ToolError, match="busy"):
                    await quick.call_tool("sparql_query", slow(0), raise_on_error=True)
                busy_after = time.monotonic() - started
                await first
                # After: a task holds the long handle, a quick call is not held up.
                task = await call_tool_task(c, "sparql_query", slow(2.5))
                await asyncio.sleep(0.5)
                started = time.monotonic()
                doc = tool_json(await quick.call_tool("sparql_query", slow(0)))
                quick_seconds = time.monotonic() - started
                done = await task.wait(timeout=30)
                return busy_after, quick_seconds, doc, done

        busy_after, quick_seconds, doc, done = asyncio.run(go())
    print(f"\nquick call behind a quick query: Busy after {busy_after:.2f}s; "
          f"quick call beside a task: {quick_seconds * 1000:.0f} ms")
    assert busy_after >= 0.9                    # the wait that used to end in Busy
    assert quick_seconds < 0.5 and len(doc["table"]) == 4
    assert done.status == "completed"


def test_two_long_tasks_on_one_dataset_take_turns_and_the_busy_message_says_so():
    with live(tokens=[TOKEN], query_timeout_s=60, task_timeout_s=1) as base:
        make_slow(PEOPLE)

        async def go():
            async with client(base) as c:
                a = await call_tool_task(c, "sparql_query", slow(2.5), raise_on_error=False)
                await asyncio.sleep(0.5)
                b = await call_tool_task(c, "sparql_query", slow(0), raise_on_error=False)
                return await b.wait(timeout=30)

        final = asyncio.run(go())
    text = final.result["content"][0]["text"]
    assert final.result["isError"] and "another long query" in text


# --------------------------------------------------------------------------- #
# Time limits
# --------------------------------------------------------------------------- #

def test_a_task_has_its_own_time_budget():
    with live(tokens=[TOKEN], query_timeout_s=1, task_timeout_s=30) as base:
        make_slow(PEOPLE)

        async def go():
            async with client(base, mode="legacy") as quick, client(base) as c:
                with pytest.raises(ToolError, match=r"limit 1s"):
                    await quick.call_tool("sparql_query", slow(1.6))
                task = await call_tool_task(c, "sparql_query", slow(1.6))
                return await task.wait(timeout=30)

        final = asyncio.run(go())
    assert final.status == "completed" and final.result["isError"] is False


def test_a_task_over_its_budget_ends_with_a_clear_error():
    with live(tokens=[TOKEN], query_timeout_s=60, task_timeout_s=1) as base:
        make_slow(PEOPLE)

        async def go():
            async with client(base) as c:
                task = await call_tool_task(c, "sparql_query", slow(1.6), raise_on_error=False)
                return await task.wait(timeout=30)

        final = asyncio.run(go())
    assert final.result["isError"] is True and "limit 1s" in final.result["content"][0]["text"]


def test_validate_shacl_and_build_rete_run_as_tasks(server):
    shapes = """@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://ex.org/> .
ex:S a sh:NodeShape ; sh:targetSubjectsOf ex:name ; sh:property [ sh:path ex:name ; sh:minCount 1 ] ."""
    ttl = "@prefix ex: <http://ex.org/> . ex:a ex:p ex:b . ex:b ex:p ex:c ."

    async def go():
        async with client(server) as c:
            v = await call_tool_task(c, "validate_shacl", {"dataset": PEOPLE, "shapes": shapes})
            b = await call_tool_task(c, "build_rete", {"rdf": ttl, "format": "ttl"})
            return tool_json(await v.result()), tool_json(await b.result())

    shacl, built = asyncio.run(go())
    assert shacl["dataset"] == PEOPLE and "report" in shacl
    assert built["dataset"] and built["dataset"] != PEOPLE
