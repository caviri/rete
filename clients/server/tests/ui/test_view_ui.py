"""The graph view in a real browser, behind a host built on the MCP Apps
SDK's own AppBridge (the host side Claude implements), against a live
rete-server over HTTP: the page runs in a sandboxed iframe, gets the tool's
input and result, and its tool calls go through the bridge to the server.

Needs Chromium, so it runs only with RETE_UI_TEST=1, in the Playwright image:

    docker run --rm -v "$PWD":/w -w /w/clients/server -e RETE_UI_TEST=1 \\
      mcr.microsoft.com/playwright/python:v1.49.0-noble sh -c \\
      'pip install -q --break-system-packages ".[test]" playwright==1.49.0 &&
       python3 -m pytest tests/ui -q -p no:cacheprovider'

RETE_UI_SHOT=<prefix> also saves screenshots (<prefix>.png, <prefix>-…).
The host's half (AppBridge) is the bundle the file explorer commits,
experiments/rete-file-explorer/graph/host-lib.js, built from the same
package version as the page.
"""
from __future__ import annotations

import asyncio
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import time

import httpx
import pytest
import rete_graph as rete

pytestmark = pytest.mark.skipif(os.environ.get("RETE_UI_TEST") != "1", reason="set RETE_UI_TEST=1 (needs Chromium)")


def host_lib() -> str:
    """The AppBridge bundle the file explorer commits (the whole checkout must be mounted)."""
    parents = pathlib.Path(__file__).resolve().parents  # ui, tests, server, clients, the checkout
    path = len(parents) > 4 and parents[4] / "experiments" / "rete-file-explorer" / "graph" / "host-lib.js"
    if not path or not path.is_file():
        pytest.fail("experiments/rete-file-explorer/graph/host-lib.js not found: mount the whole checkout, "
                    "not only clients/server")
    return path.read_text(encoding="utf-8")


TOKEN = "ui-test-token-0123456789"
PEOPLE_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles , ex:mary ; ex:worksFor ex:engine .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" ; ex:worksFor ex:engine ; ex:born "1791" .
ex:mary a ex:Person ; rdfs:label "Mary Somerville" ; ex:knows ex:charles .
ex:engine a ex:Project ; rdfs:label "Analytical Engine" .
ex:lab a ex:Org ; rdfs:label "Lab" ; ex:hosts ex:engine .
"""
KNOWS = ("PREFIX ex: <https://example.org/> CONSTRUCT { ?a ex:knows ?b . ?a a ?t } "
         "WHERE { ?a ex:knows ?b ; a ?t }")
P = lambda k: f"<https://example.org/{k}>"  # noqa: E731


@pytest.fixture(scope="module")
def server():
    root = pathlib.Path(tempfile.mkdtemp(prefix="rete-server-ui-"))
    (root / "data").mkdir()
    (root / "data" / "people.rete").write_bytes(rete.build(PEOPLE_TTL, "ttl"))
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    env = {**os.environ, "RETE_TOKEN": TOKEN, "RETE_GENERATED_DIR": str(root / "generated")}
    proc = subprocess.Popen(
        [sys.executable, "-m", "rete_server", "--host", "127.0.0.1", "--port", str(port), "--catalog", "none",
         "--data", str(root / "data"), "--log-level", "warning"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
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
        yield {"base": base, "people": (root / "data" / "people.rete").read_bytes()}
    finally:
        proc.terminate()
        proc.wait(timeout=15)


class Host:
    """What Claude does with the server: MCP calls over HTTP, and the page in
    a sandboxed iframe behind an AppBridge."""

    def __init__(self, base: str, page):
        self.base = base
        self.page = page
        self.http = httpx.AsyncClient(timeout=60, headers={
            "Authorization": f"Bearer {TOKEN}", "Accept": "application/json, text/event-stream"})
        self.messages: list = []
        self.context: list = []
        self.ids = 0

    async def rpc(self, method: str, params: dict) -> dict:
        self.ids += 1
        r = await self.http.post(f"{self.base}/mcp", json={"jsonrpc": "2.0", "id": self.ids, "method": method,
                                                           "params": params})
        r.raise_for_status()
        doc = r.json()
        assert "error" not in doc, doc
        return doc["result"]

    async def tool(self, name: str, args: dict) -> dict:
        return await self.rpc("tools/call", {"name": name, "arguments": args})

    async def data(self, name: str, args: dict) -> dict:
        return json.loads((await self.tool(name, args))["content"][0]["text"])

    async def open(self, args: dict):
        """Claude calls graph_view; the host shows the tool's page with its input and result."""
        html = (await self.rpc("resources/read", {"uri": "ui://rete/graph-view.html"}))["contents"][0]["text"]
        result = await self.tool("graph_view", args)
        assert not result.get("isError"), result
        await self.page.expose_function("hostCallTool", lambda params: self.tool(params["name"],
                                                                                  params.get("arguments") or {}))
        await self.page.expose_function("hostSaw", lambda kind, params: (
            self.messages if kind == "message" else self.context).append(params))
        # A page of its own origin, so the host's half loads as a module.
        await self.page.route("https://host.test/host-lib.js", lambda route: route.fulfill(
            body=host_lib(), content_type="text/javascript"))
        await self.page.route("https://host.test/", lambda route: route.fulfill(
            content_type="text/html",
            body='<!doctype html><html><body style="margin:0"><iframe id="f" sandbox="allow-scripts" '
                 'style="border:0;width:1080px;height:600px"></iframe></body></html>'))
        await self.page.goto("https://host.test/")
        await self.page.evaluate("""async ({ html, args, result }) => {
            const { AppBridge, PostMessageTransport } = await import("/host-lib.js");
            const iframe = document.getElementById("f");
            const bridge = new AppBridge(
              null,
              { name: "test host", version: "0" },
              { openLinks: {}, serverTools: {}, updateModelContext: { text: {} }, message: { text: {} } },
              { hostContext: { theme: "light", displayMode: "inline", availableDisplayModes: ["inline"],
                               platform: "web" } },
            );
            bridge.oncalltool = (params) => window.hostCallTool(params);
            bridge.onupdatemodelcontext = async (params) => (window.hostSaw("context", params), {});
            bridge.onmessage = async (params) => (window.hostSaw("message", params), {});
            bridge.onrequestdisplaymode = async ({ mode }) => ({ mode });
            bridge.onopenlink = async () => ({});
            bridge.oninitialized = () => { bridge.sendToolInput({ arguments: args }); bridge.sendToolResult(result); };
            await bridge.connect(new PostMessageTransport(iframe.contentWindow, iframe.contentWindow));
            iframe.srcdoc = html;
        }""", {"html": html, "args": args, "result": result})
        for _ in range(50):
            self.frame = next((f for f in self.page.frames if f != self.page.main_frame), None)
            if self.frame:
                break
            await asyncio.sleep(0.1)
        self.view = self.page.frame_locator("#f")
        return result["structuredContent"]["view_id"]

    async def until(self, js: str, arg=None, what: str = "", tries: int = 150):
        for _ in range(tries):
            try:
                if await self.frame.evaluate(js, arg):
                    return
            except Exception:
                pass
            await asyncio.sleep(0.1)
        pytest.fail(f"timed out waiting for {what}")

    async def shot(self, suffix: str = ""):
        prefix = os.environ.get("RETE_UI_SHOT")
        if prefix:
            await asyncio.sleep(0.6)
            await self.page.locator("#f").screenshot(path=f"{prefix.removesuffix('.png')}{suffix}.png")


async def _with_host(base: str, body):
    from playwright.async_api import async_playwright

    async with async_playwright() as pw:
        browser = await pw.chromium.launch()
        try:
            page = await browser.new_page(viewport={"width": 1100, "height": 640})
            host = Host(base, page)
            try:
                await body(host)
            finally:
                await host.http.aclose()
        finally:
            await browser.close()


def test_the_graph_view_runs_against_rete_server(server):
    async def body(h: Host):
        view_id = await h.open({"dataset": "local/people.rete", "query": KNOWS, "title": "who knows whom"})

        # It loads its view (graph_view_sync), then names the nodes (graph_view_labels).
        await h.until("() => window.__reteView?.cy?.nodes().length === 3", what="3 nodes drawn")
        await h.until("(id) => window.__reteView.cy.getElementById(id).data('label') === 'Ada Lovelace'",
                      P("ada"), "labels")
        assert await h.frame.evaluate("() => window.__reteView.viewId") == view_id

        # Selecting reports to the server, and Claude reads it with graph_view_state.
        await h.frame.evaluate("(id) => window.__reteView.cy.getElementById(id).select()", P("ada"))
        for _ in range(50):
            st = await h.data("graph_view_state", {"view_id": view_id})
            if st["user"].get("selection"):
                break
            await asyncio.sleep(0.1)
        assert st["user"]["open"] is True
        assert st["user"]["selection"][0]["label"] == "Ada Lovelace"
        await h.frame.evaluate("() => window.__reteView.cy.$(':selected').unselect()")

        # Claude changes the open view; the page picks the patch up on its next sync.
        await h.data("graph_view_update", {
            "view_id": view_id, "options": {"layout": "circle", "edge_labels": True},
            "query": "PREFIX ex: <https://example.org/> CONSTRUCT { ?p ex:worksFor ?o } WHERE { ?p ex:worksFor ?o }"})
        await h.until("() => window.__reteView.options.layout === 'circle' && window.__reteView.cy.nodes().length === 4",
                      what="Claude's update")

        # Double-click expands a node (graph_view_expand): ex:lab hosts the engine.
        await h.frame.evaluate("(id) => window.__reteView.cy.getElementById(id).emit('dbltap')", P("engine"))
        await h.until("(id) => window.__reteView.cy.getElementById(id).nonempty()", P("lab"), "the expansion")

        # Enter in the search box finds by name in the dataset (graph_view_search).
        await h.view.locator("#search").fill("Lab")
        await h.view.locator("#search").press("Enter")
        await h.until("() => /found in the graph/.test(document.getElementById('status-note').textContent)",
                      what="the search")
        await h.shot()

        # The user's edits go to the server (graph_view_edit): Remove, from a node's menu.
        await h.frame.evaluate("""(id) => window.__reteView.cy.getElementById(id)
            .emit('cxttap')""", P("mary"))
        await h.until("() => !document.getElementById('ctx').hidden", what="the node's menu")
        await h.view.locator("#ctx button", has_text="Remove").click()
        await h.until("(id) => !window.__reteView.nodes.has(id)", P("mary"), "Mary removed")
        assert (await h.data("graph_view_state", {"view_id": view_id}))["nodes"] == 4

    asyncio.run(_with_host(server["base"], body))


def test_a_schema_draws_as_class_boxes(server):
    async def body(h: Host):
        await h.open({"dataset": "local/people.rete", "schema": True})
        await h.until("() => window.__reteView.cy?.nodes('.card').length === 3", what="a box per class")
        assert await h.view.locator("#opt-diagram").input_value() == "boxes"
        person = await h.frame.evaluate(
            "(id) => decodeURIComponent(window.__reteView.cy.getElementById(id).data('svg'))", P("Person"))
        assert ">3 instances<" in person and ">label<" in person
        await h.shot("-schema")

    asyncio.run(_with_host(server["base"], body))


def test_a_file_attached_to_the_chat_opens_when_the_user_chooses_it(server):
    async def body(h: Host):
        await h.open({"dataset": "/mnt/user-data/uploads/talks.rete"})

        # Claude's copy is out of reach: the view asks the user for the file.
        await h.until("() => !document.getElementById('awaiting').hidden", what="the request for the file")
        assert await h.view.locator("#awaiting-file").text_content() == "talks.rete"
        await h.shot("-attachment")

        # The user chooses it (graph_view_upload, in parts); it becomes a dataset here.
        await h.view.locator("#file-input").set_input_files(
            {"name": "talks.rete", "mimeType": "application/octet-stream", "buffer": server["people"]})
        await h.until("() => (window.__reteView.dataset || '').startsWith('generated/') "
                      "&& window.__reteView.cy.nodes().length > 0", what="the file drawn")
        assert await h.view.locator("#awaiting").is_hidden()
        # Claude asked for it, so the view tells Claude, which carries on with that dataset.
        for _ in range(30):
            if h.messages:
                break
            await asyncio.sleep(0.1)
        text = h.messages[-1]["content"][0]["text"]
        assert text.startswith("I opened talks.rete in graph view v"), text
        dataset = text.split('dataset "', 1)[1].split('"', 1)[0]
        assert "(17 statements)" in text
        count = await h.data("sparql_query", {"dataset": dataset, "query": "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"})
        assert count["table"] == [{"n": 17}]

    asyncio.run(_with_host(server["base"], body))
