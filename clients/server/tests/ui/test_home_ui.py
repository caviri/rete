"""The home page in a real browser against a live rete-server: the token gate,
a file dropped on the drop zone becoming a dataset with rows, phone width, and
the WebMCP tools registered with a stub ``navigator.modelContext``.

Needs Chromium, so it runs only with RETE_UI_TEST=1, in the Playwright image:

    docker run --rm -v "$PWD":/w -w /w/clients/server -e RETE_UI_TEST=1 \\
      mcr.microsoft.com/playwright/python:v1.49.0-noble sh -c \\
      'pip install -q --break-system-packages ".[test]" playwright==1.49.0 &&
       python3 -m pytest tests/ui -q -p no:cacheprovider'

RETE_HOME_SHOTS=<dir> also saves screenshots there.
"""
from __future__ import annotations

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

TOKEN = "home-ui-token-0123456789"
PEOPLE_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" .
"""
DROPPED_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:rete a ex:Format ; rdfs:label "rete" ; ex:readBy ex:server , ex:browser .
ex:server a ex:Reader ; rdfs:label "rete-server" .
ex:browser a ex:Reader ; rdfs:label "browser" .
"""
STUB = """
window.__registered = [];
Object.defineProperty(navigator, "modelContext", { configurable: true, value: {
  registerTool(tool, opts) { window.__registered.push({ tool, signal: !!(opts && opts.signal) }); }
} });
"""


@pytest.fixture(scope="module")
def server():
    root = pathlib.Path(tempfile.mkdtemp(prefix="rete-home-ui-"))
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
        yield base
    finally:
        proc.terminate()
        proc.wait(timeout=15)


@pytest.fixture(scope="module")
def browser():
    from playwright.sync_api import sync_playwright

    with sync_playwright() as pw:
        b = pw.chromium.launch()
        yield b
        b.close()


def shot(page, name: str):
    out = os.environ.get("RETE_HOME_SHOTS")
    if out:
        pathlib.Path(out).mkdir(parents=True, exist_ok=True)
        time.sleep(0.5)
        page.screenshot(path=str(pathlib.Path(out) / f"{name}.png"), full_page=True)


def drop_file(page, name: str, text: str, target: str = "#drop"):
    """What a file manager does: a drop event whose dataTransfer carries a File."""
    page.evaluate("""([sel, name, text]) => {
        const dt = new DataTransfer();
        dt.items.add(new File([text], name, { type: "text/turtle" }));
        const el = document.querySelector(sel);
        for (const type of ["dragenter", "dragover", "drop"]) {
          el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }));
        }
    }""", [target, name, text])


def test_gate_then_listing_without_the_token_in_the_page(server, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(server + "/")
    page.wait_for_selector("#gate:not([hidden])")
    assert page.locator("#app").is_hidden()
    # a wrong token is refused, the right one opens the page
    page.fill("#tokenInput", "nope")
    page.click("#gateForm button")
    page.wait_for_function("document.getElementById('gateMsg').textContent.includes('not accepted')")
    page.fill("#tokenInput", TOKEN)
    page.click("#gateForm button")
    page.wait_for_selector("#app:not([hidden])")
    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    assert "1 dataset" in page.locator("#stats").inner_text() or "datasets" in page.locator("#stats").inner_text()
    # the token is kept for this tab only
    assert page.evaluate("sessionStorage.getItem('rete.home.token')") == TOKEN
    assert page.evaluate("localStorage.length") == 0
    page.reload()
    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    assert not errors, errors


def test_token_in_the_url_is_taken_and_removed(server, browser):
    page = browser.new_page()
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    assert "token" not in page.url
    assert page.locator("#gate").is_hidden()


def test_drop_a_file_builds_a_dataset_and_shows_rows(server, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    # pick a catalog dataset first: its card and the starter queries
    page.click("#dlist li[data-key='local/people.rete']")
    page.wait_for_selector("#cardPanel .facts")
    page.click("#starters .chip >> nth=-3")
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() >= 1

    drop_file(page, "formats.ttl", DROPPED_TTL)
    page.wait_for_function("document.getElementById('current').textContent.startsWith('generated/')", timeout=30000)
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() == 8  # the starter query: all 8 triples
    assert page.locator("#dlist li[aria-selected='true'] .t").inner_text() == "formats.ttl"
    assert "formats.ttl" in page.locator("#cardPanel h3").inner_text()
    shot(page, "desktop-after-drop")

    # a file the server cannot read says so and changes nothing
    current = page.locator("#current").inner_text()
    drop_file(page, "junk.ttl", "this is {{{ not turtle")
    page.wait_for_selector("#toast.bad")
    assert "junk.ttl" in page.locator("#toast").inner_text()
    assert page.locator("#current").inner_text() == current
    assert not errors, errors


def test_phone_width_has_no_horizontal_scroll(server, browser):
    page = browser.new_page(viewport={"width": 390, "height": 844}, device_scale_factor=2)
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    page.click("#dlist li[data-key='local/people.rete']")
    page.wait_for_selector("#cardPanel .facts")
    drop_file(page, "formats.ttl", DROPPED_TTL)
    page.wait_for_selector("#result table")
    shot(page, "phone-after-drop")
    assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth")
    assert page.evaluate("document.body.scrollWidth <= window.innerWidth")


def test_webmcp_tools_register_and_run(server, browser):
    ctx = browser.new_context(viewport={"width": 1280, "height": 900})
    ctx.add_init_script(STUB)
    page = ctx.new_page()
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_function("window.reteWebMCP")
    reg = page.evaluate("""() => ({ ann: window.reteWebMCP,
        tools: window.__registered.map((r) => ({ name: r.tool.name, signal: r.signal, type: r.tool.inputSchema.type,
                                                exec: typeof r.tool.execute })) })""")
    names = {t["name"] for t in reg["tools"]}
    assert reg["ann"]["supported"] is True and set(reg["ann"]["tools"]) == names
    assert {"list_datasets", "describe_dataset", "get_dataset_card", "get_schema", "list_examples", "run_sparql",
            "find_entities", "open_dataset", "show_query", "upload_info", "get_activity", "get_state"} <= names
    assert all(t["signal"] and t["type"] == "object" and t["exec"] == "function" for t in reg["tools"])

    page.wait_for_selector("#dlist li[data-key='local/people.rete']")
    # called the way a browser agent calls it: through the registered tool
    listed = page.evaluate("""async () => {
        const t = window.__registered.find((r) => r.tool.name === "list_datasets").tool;
        const p = t.execute({ query: "people" });
        const label = document.getElementById("pill").textContent;   // shown while it runs
        return { label, out: await p };
    }""")
    assert "list_datasets" in listed["label"]
    assert [d["key"] for d in listed["out"]["datasets"]] == ["local/people.rete"]
    # open_dataset changes the page the person sees; run_sparql returns data and leaves it alone
    page.evaluate("window.reteHomeTools.call('open_dataset', { key: 'local/people.rete' })")
    assert page.locator("#current").inner_text() == "local/people.rete"
    out = page.evaluate("""window.reteHomeTools.call('run_sparql', {
        query: 'SELECT ?s WHERE { ?s a <https://example.org/Person> }', limit: 10 })""")
    assert out["kind"] == "select" and out["total"] == 2 and out["vars"] == ["s"]
    schema = page.evaluate("window.reteHomeTools.call('get_schema', {})")
    assert schema["classes"][0][0] == "https://example.org/Person"
    hits = page.evaluate("window.reteHomeTools.call('find_entities', { prefix: 'Ada' })")
    assert any(m["label"] == "Ada Lovelace" for m in hits["matches"])
    bad = page.evaluate("""window.reteHomeTools.call('open_dataset', { key: 'nope' }).then(() => null, (e) => e.message)""")
    assert "No dataset" in bad
    activity = page.evaluate("window.reteHomeTools.call('get_activity', {})")
    assert [c["tool"] for c in activity["calls"]][:3] == ["find_entities", "get_schema", "run_sparql"] or activity["total"] >= 5
    assert any(c["state"] == "error" for c in activity["calls"])
    assert "#pill" and page.locator("#pill").is_visible()
    # the page tells a person what it offers
    assert "WebMCP tools" in page.locator("#webmcpNote").inner_text()


def test_no_webmcp_is_a_clean_no_op(server, browser):
    page = browser.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_function("window.reteWebMCP")
    assert page.evaluate("window.reteWebMCP.supported") is False
    assert page.evaluate("window.reteHomeTools.list().length") >= 10
    assert not errors, errors
