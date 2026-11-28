"""The home page in a real browser against a live rete-server: open mode (no token,
authoring off), token mode, uploads on, a file dropped on the drop zone becoming a
graph with rows, phone width, and the WebMCP tools registered with a stub
``navigator.modelContext``.

Needs Chromium, so it runs only with RETE_UI_TEST=1, in the Playwright image:

    docker run --rm -v "$PWD":/w -w /w/clients/server -e RETE_UI_TEST=1 \\
      mcr.microsoft.com/playwright/python:v1.49.0-noble sh -c \\
      'pip install -q --break-system-packages ".[test]" playwright==1.49.0 &&
       python3 -m pytest tests/ui -q -p no:cacheprovider'

RETE_HOME_SHOTS=<dir> also saves screenshots there.
"""
from __future__ import annotations

import contextlib
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

TOKEN = "home-ui-token-0123456789"
PEOPLE_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" .
"""
JAZZ_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:Miles_Davis a ex:Musician ; rdfs:label "Miles Davis" ; ex:albums 1500 ; ex:influenced ex:John_Coltrane .
ex:John_Coltrane a ex:Musician ; rdfs:label "John Coltrane" ; ex:albums 1234567 .
"""
DROPPED_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:rete a ex:Format ; rdfs:label "rete" ; ex:readBy ex:server , ex:browser .
ex:server a ex:Reader ; rdfs:label "rete-server" .
ex:browser a ex:Reader ; rdfs:label "browser" .
"""
PREFIX = "PREFIX ex: <https://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
STARTER_TITLE = "Who played most?"
CATALOG = {"datasets": [
    {"key": "jazz-mini", "label": "jazz-mini.rete - a tiny jazz graph (remote, lazy)",
     "description": "A **tiny** jazz graph for tests. It has two musicians and one link.", "url": "jazz.rete",
     "triples": 5, "size": "1 KB", "tags": ["music", "test"], "license": "CC0",
     "examples": [{"title": STARTER_TITLE, "question": "Musicians by number of albums.",
                   "sparql": PREFIX + "SELECT ?who ?albums WHERE { ?who ex:albums ?albums } ORDER BY DESC(?albums)"}]},
    {"key": "books-mini", "label": "books-mini.rete - a tiny library", "description": "Two books.",
     "url": "books.rete", "triples": 3, "size": "1 KB", "tags": ["books", "test"]},
]}
STUB = """
window.__registered = [];
Object.defineProperty(navigator, "modelContext", { configurable: true, value: {
  registerTool(tool, opts) { window.__registered.push({ tool, signal: !!(opts && opts.signal) }); }
} });
"""


@contextlib.contextmanager
def run_server(*, token: str | None = None, author: bool = True, catalog: bool = True):
    root = pathlib.Path(tempfile.mkdtemp(prefix="rete-home-ui-"))
    (root / "data").mkdir()
    (root / "data" / "people.rete").write_bytes(rete.build(PEOPLE_TTL, "ttl"))
    args = []
    if catalog:
        (root / "jazz.rete").write_bytes(rete.build(JAZZ_TTL, "ttl"))
        (root / "books.rete").write_bytes(rete.build(PEOPLE_TTL, "ttl"))
        (root / "catalog.json").write_text(json.dumps(CATALOG), encoding="utf-8")
        args = ["--catalog", str(root / "catalog.json")]
    else:
        args = ["--catalog", "none"]
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    env = {k: v for k, v in os.environ.items() if not k.startswith("RETE_")}
    env.update({"RETE_GENERATED_DIR": str(root / "generated"), "RETE_AUTHOR": "1" if author else "0"})
    if token:
        env["RETE_TOKEN"] = token
    proc = subprocess.Popen(
        [sys.executable, "-m", "rete_server", "--host", "127.0.0.1", "--port", str(port), *args,
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
def server():
    """Token mode, uploads on, a two-entry catalog plus one local file."""
    with run_server(token=TOKEN) as base:
        yield base


@pytest.fixture(scope="module")
def open_off():
    """What the public Space looks like: no token, authoring off."""
    with run_server(author=False) as base:
        yield base


@pytest.fixture(scope="module")
def open_on():
    """No token, uploads on, no catalog (only a local file)."""
    with run_server(catalog=False) as base:
        yield base


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
        time.sleep(0.6)
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


def ready(page):
    page.wait_for_function("window.reteHome && window.reteHome.state.open && !window.__homeBooting")


def no_scroll_x(page):
    assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth")
    assert page.evaluate("document.body.scrollWidth <= window.innerWidth")


# --------------------------------------------------------------------------- #
# Token mode
# --------------------------------------------------------------------------- #

def test_gate_then_listing_without_the_token_in_the_page(server, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(server + "/")
    page.wait_for_selector("#gate:not([hidden])")
    assert page.locator("#app").is_hidden()
    shot(page, "token-gate")
    # a wrong token is refused, the right one opens the page
    page.fill("#tokenInput", "nope")
    page.click("#gateForm button")
    page.wait_for_function("document.getElementById('gateMsg').textContent.includes('not accepted')")
    page.fill("#tokenInput", TOKEN)
    page.click("#gateForm button")
    page.wait_for_selector("#app:not([hidden])")
    page.wait_for_selector("li[data-key='jazz-mini']")
    # the token UI is there in token mode
    assert page.locator("#signOut").is_visible() and page.locator("#tokenOpts").is_visible()
    # the token is kept for this tab only
    assert page.evaluate("sessionStorage.getItem('rete.home.token')") == TOKEN
    assert page.evaluate("localStorage.length") == 0
    page.reload()
    page.wait_for_selector("li[data-key='jazz-mini']")
    assert not errors, errors


def test_token_in_the_url_is_taken_and_removed(server, browser):
    page = browser.new_page()
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_selector("li[data-key='jazz-mini']")
    assert "token" not in page.url
    assert page.locator("#gate").is_hidden()
    # the connect tab says how the token is sent; reveal shows it
    assert "Authorization: Bearer <token>" in page.locator("#mcpCmd").inner_text()
    page.check("#reveal")
    assert TOKEN in page.locator("#mcpCmd").inner_text()
    page.click("#queryTab")
    assert f"/mcp?token={TOKEN}" in page.locator("#mcpUrl").inner_text()


def test_drop_a_file_builds_a_dataset_and_shows_rows(server, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_selector("li[data-key='jazz-mini']")
    assert page.locator("#drop").is_visible() and page.locator("#byoOff").is_hidden()
    # pick a catalog graph first: its card and the starter questions
    page.click("li[data-key='jazz-mini'] button")
    page.wait_for_selector("#cardPanel .facts")
    page.click(f"#starters button:has-text('{STARTER_TITLE}')")
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() == 2

    drop_file(page, "formats.ttl", DROPPED_TTL)
    page.wait_for_function("document.getElementById('current').textContent.startsWith('generated/')", timeout=30000)
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() == 8  # the starter query: all 8 triples
    assert page.locator("#llist li.on .t").inner_text() == "formats.ttl"
    assert "formats.ttl" in page.locator("#cardPanel h3").inner_text().lower()
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
    page.wait_for_selector("li[data-key='jazz-mini']")
    page.click("li[data-key='jazz-mini'] button")
    page.wait_for_selector("#cardPanel .facts")
    drop_file(page, "formats.ttl", DROPPED_TTL)
    page.wait_for_selector("#result table")
    shot(page, "phone-after-drop")
    no_scroll_x(page)


# --------------------------------------------------------------------------- #
# Open mode (no token, authoring off): the public Space
# --------------------------------------------------------------------------- #

def test_open_mode_shows_no_token_ui_and_a_friendly_byo_panel(open_off, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(open_off + "/")
    ready(page)
    page.wait_for_selector("#byoOff:not([hidden])")
    # the new headline, the three steps, no token anywhere
    assert "Your RDF graphs in one place" in page.locator("h1").inner_text()
    assert page.locator(".steps li").count() == 3
    assert page.locator("#gate").is_hidden() and page.locator("#signOut").is_hidden() and page.locator("#tokenOpts").is_hidden()
    assert "token" not in page.locator("#connect").inner_text().lower()
    assert "token" not in page.locator("#stats").inner_text().lower()
    assert "Authorization" not in page.locator("#mcpCmd").inner_text()
    assert page.locator("#mcpCmd").inner_text() == f"claude mcp add --transport http rete {open_off}/mcp"
    # uploads are off: no drop zone, a panel that says where to go
    assert page.locator("#drop").is_hidden() and page.locator("#byoOn").is_hidden()
    assert page.locator("#addTitle").inner_text() == "Bring your own graph"
    assert page.locator("a[href='https://caviri.github.io/rete/playground.html']").is_visible()
    assert page.locator("#dockerCode").inner_text() == "docker run -p 7860:7860 ghcr.io/caviri/rete-server"
    # catalog first, files on this server collapsed
    assert page.locator("li[data-key='jazz-mini']").count() + page.locator(".fcard[data-key='jazz-mini']").count() >= 1
    assert page.locator("#localBox").get_attribute("open") is None
    assert "Files on this server (1)" in page.locator("#localSum").inner_text()
    page.click("#localSum")
    assert page.locator("#llist li[data-key='local/people.rete']").is_visible()
    shot(page, "open-off-desktop")
    assert not errors, errors


def test_open_mode_try_it_then_starter_question_shows_rows(open_off, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(open_off + "/")
    ready(page)
    page.wait_for_selector(".fcard[data-key='jazz-mini']")
    page.click(".fcard[data-key='jazz-mini'] button")
    page.wait_for_selector(f"#starters button:has-text('{STARTER_TITLE}')")
    assert page.locator("#current").inner_text() == "jazz-mini"
    # the SPARQL is secondary: closed until asked for
    assert page.locator("#sparqlBox").get_attribute("open") is None
    page.click(f"#starters button:has-text('{STARTER_TITLE}')")
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() == 2
    # IRIs show their local name with the full IRI as the link; numbers are formatted
    link = page.locator("#result tbody tr >> nth=0").locator("a.iri")
    assert link.inner_text() == "John Coltrane" and link.get_attribute("href") == "https://example.org/John_Coltrane"
    assert link.get_attribute("title") == "https://example.org/John_Coltrane"
    assert "1,234,567" in page.locator("#result tbody tr >> nth=0").inner_text()
    assert "rows" in page.locator("#result .table-foot").inner_text()
    page.click("#sparqlBox summary")
    assert "SELECT ?who" in page.locator("#q").input_value()
    shot(page, "open-off-query")

    # nothing matched: said plainly
    page.evaluate("window.reteHome.run(\"SELECT ?s WHERE { ?s a <https://example.org/Nothing> }\")")
    page.wait_for_selector("#result .none-msg")
    assert "No rows" in page.locator("#result").inner_text()
    # a broken query: plain words and the server's message
    page.evaluate("window.reteHome.run('SELECT ?s WHERE {').catch(() => null)")
    page.wait_for_selector("#result .err")
    err = page.locator("#result .err").inner_text()
    assert "did not run" in err and len(err) > 40
    assert not errors, errors


def test_open_mode_search_and_tag_filter(open_off, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    page.goto(open_off + "/")
    ready(page)
    page.wait_for_selector("#tagChips button")
    page.click("#tagChips button:has-text('test')")
    assert page.locator("#tagChips button:has-text('test')").get_attribute("aria-pressed") == "true"
    assert page.locator("#featuredBox").is_hidden()  # searching replaces the quick picks
    assert page.locator("#dlist li").count() == 2
    page.click("#tagChips button:has-text('test')")  # off again
    page.fill("#search", "jazz")
    assert page.locator("li[data-key='jazz-mini']").count() == 1
    assert page.locator("#dlist li[data-key='books-mini']").count() == 0
    page.fill("#search", "zzzz-nothing")
    assert "Nothing matches" in page.locator("#dlist").inner_text()


def test_connect_tabs_and_copy(open_off, browser):
    ctx = browser.new_context(viewport={"width": 1280, "height": 900})
    ctx.grant_permissions(["clipboard-read", "clipboard-write"], origin=open_off)
    page = ctx.new_page()
    page.goto(open_off + "/")
    ready(page)
    for tab, pane, text in [("claude-ai", "cUrl", f"{open_off}/mcp"), ("chatgpt", "gUrl", f"{open_off}/mcp"),
                            ("python", "pyCode", "from fastmcp import Client")]:
        page.click(f"#tab-{tab}")
        assert page.locator(f"#pane-{tab}").is_visible()
        assert text in page.locator(f"#{pane}").inner_text()
        assert "token" not in page.locator(f"#pane-{tab}").inner_text().lower()
    page.click("#tab-claude-code")
    page.click("#pane-claude-code [data-copy]")
    assert page.evaluate("navigator.clipboard.readText()") == f"claude mcp add --transport http rete {open_off}/mcp"
    # arrow keys move between tabs
    page.focus("#tab-claude-code")
    page.keyboard.press("ArrowRight")
    assert page.locator("#tab-claude-ai").get_attribute("aria-selected") == "true"


def test_open_mode_phone_has_no_horizontal_scroll_and_toast_stays_clear(open_off, browser):
    page = browser.new_page(viewport={"width": 390, "height": 844}, device_scale_factor=2)
    page.goto(open_off + "/")
    ready(page)
    shot(page, "open-off-phone-top")
    page.click(".fcard[data-key='jazz-mini'] button")
    page.wait_for_selector(f"#starters button:has-text('{STARTER_TITLE}')")
    page.click(f"#starters button:has-text('{STARTER_TITLE}')")
    page.wait_for_selector("#result table")
    page.evaluate("window.reteHome.toast('Something to say', true)")
    box = page.locator("#toast").bounding_box()
    assert box["y"] < 120  # at the top, not over the results
    shot(page, "open-off-phone-query")
    no_scroll_x(page)
    page.evaluate("document.getElementById('add').scrollIntoView()")
    shot(page, "open-off-phone-byo")
    no_scroll_x(page)


def test_reduced_motion_stops_the_rotator(open_off, browser):
    ctx = browser.new_context(reduced_motion="reduce")
    page = ctx.new_page()
    page.goto(open_off + "/")
    ready(page)
    assert page.locator("#rot.still").count() == 1
    time.sleep(2.8)
    assert page.locator("#rot .rot-word").inner_text() == "a browser"


# --------------------------------------------------------------------------- #
# Uploads on
# --------------------------------------------------------------------------- #

def test_open_mode_with_uploads_keeps_the_drop_zone(open_on, browser):
    page = browser.new_page(viewport={"width": 1280, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(open_on + "/")
    ready(page)
    page.wait_for_selector("#byoOn:not([hidden])")
    assert page.locator("#byoOff").is_hidden() and page.locator("#drop").is_visible()
    assert page.locator("#tokenOpts").is_hidden() and "Authorization" not in page.locator("#curlCode").inner_text()
    # no catalog: the files group is open by itself
    assert page.locator("#localBox").get_attribute("open") is not None
    shot(page, "open-on-desktop")
    drop_file(page, "formats.ttl", DROPPED_TTL)
    page.wait_for_function("document.getElementById('current').textContent.startsWith('generated/')", timeout=30000)
    page.wait_for_selector("#result table")
    assert page.locator("#result tbody tr").count() == 8
    shot(page, "open-on-after-drop")
    assert not errors, errors
    phone = browser.new_page(viewport={"width": 390, "height": 844}, device_scale_factor=2)
    phone.goto(open_on + "/")
    ready(phone)
    phone.wait_for_selector("#byoOn:not([hidden])")
    phone.evaluate("document.getElementById('add').scrollIntoView()")
    shot(phone, "open-on-phone-byo")
    no_scroll_x(phone)


# --------------------------------------------------------------------------- #
# WebMCP
# --------------------------------------------------------------------------- #

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

    ready(page)
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
    shown = page.evaluate("""window.reteHomeTools.call('show_query', { query: 'SELECT ?s WHERE { ?s a <https://example.org/Person> }' })""")
    assert shown["shown"] is True
    assert page.locator("#result tbody tr").count() == 2
    activity = page.evaluate("window.reteHomeTools.call('get_activity', {})")
    assert activity["total"] >= 5
    assert any(c["state"] == "error" for c in activity["calls"])
    assert page.locator("#pill").is_visible()
    # the page tells a person what it offers
    assert "WebMCP tools" in page.locator("#webmcpNote").inner_text()


def test_webmcp_in_open_mode(open_off, browser):
    ctx = browser.new_context()
    ctx.add_init_script(STUB)
    page = ctx.new_page()
    page.goto(open_off + "/")
    page.wait_for_function("window.reteWebMCP")
    ready(page)
    info = page.evaluate("window.reteHomeTools.call('upload_info', {})")
    assert info["enabled"] is False and "Authorization" not in info["curl"]
    state = page.evaluate("window.reteHomeTools.call('get_state', {})")
    assert state["uploads"] is False
    listed = page.evaluate("window.reteHomeTools.call('list_datasets', { query: 'jazz' })")
    assert [d["key"] for d in listed["datasets"]] == ["jazz-mini"]


def test_no_webmcp_is_a_clean_no_op(server, browser):
    page = browser.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"{server}/?token={TOKEN}")
    page.wait_for_function("window.reteWebMCP")
    assert page.evaluate("window.reteWebMCP.supported") is False
    assert page.evaluate("window.reteHomeTools.list().length") >= 10
    assert not errors, errors
