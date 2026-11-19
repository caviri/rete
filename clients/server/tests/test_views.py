"""The graph view (an MCP App), at the protocol level: what the host sees in
tools/list and resources/read, and the tools the page and Claude call. A port
of clients/mcpb/test/view.test.mjs: the same page talks to both servers, so
both answer the same way. The page itself, in a browser: tests/ui/.
"""
from __future__ import annotations

import asyncio
import base64
import json
import pathlib
import tempfile

import pytest
import rete_graph as rete

from conftest import serve

PEOPLE_TTL = """\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <https://example.org/> .
ex:ada a ex:Person ; rdfs:label "Ada Lovelace" ; ex:knows ex:charles , ex:mary ; ex:worksFor ex:engine .
ex:charles a ex:Person ; rdfs:label "Charles Babbage" ; ex:worksFor ex:engine ; ex:born "1791" .
ex:mary a ex:Person ; rdfs:label "Mary Somerville" ; ex:knows ex:charles .
ex:engine a ex:Project ; rdfs:label "Analytical Engine" .
ex:lab a ex:Org ; rdfs:label "Lab" ; ex:hosts ex:engine .
"""
VIEW_DATA = pathlib.Path(tempfile.mkdtemp(prefix="rete-server-views-"))
PEOPLE_BYTES = rete.build(PEOPLE_TTL, "ttl")
(VIEW_DATA / "people.rete").write_bytes(PEOPLE_BYTES)
PEOPLE = "local/people.rete"
EDGES = "PREFIX ex: <https://example.org/> CONSTRUCT { ?a ex:knows ?b } WHERE { ?a ex:knows ?b }"
P = lambda k: f"<https://example.org/{k}>"  # noqa: E731


class Session:
    """An MCP client on the app's own server object, with the calls the tests make."""

    def __init__(self, client):
        self.c = client

    async def call(self, name, args):
        r = await self.c.call_tool(name, args, raise_on_error=False)
        assert not r.is_error, f"{name} errored: {r.content[0].text}"
        return r

    async def data(self, name, args):
        return json.loads((await self.call(name, args)).content[0].text)

    async def refusal(self, name, args):
        r = await self.c.call_tool(name, args, raise_on_error=False)
        assert r.is_error, f"{name} should have failed"
        return r.content[0].text


def run(test, **overrides):
    """Run ``test(session)`` against a fresh server over VIEW_DATA."""
    from fastmcp import Client

    from rete_server.mcp import build_mcp

    async def go():
        async with Client(build_mcp(author=overrides.get("author", True))) as c:
            return await test(Session(c))

    with serve(catalogs=[], data_dir=VIEW_DATA, **overrides):
        return asyncio.run(go())


def test_the_host_sees_a_ui_tool_its_resource_and_app_only_tools():
    async def t(s):
        tools = {x.name: x for x in await s.c.list_tools()}
        assert tools["graph_view"].meta["ui"]["resourceUri"] == "ui://rete/graph-view.html"
        for name in ("graph_view_sync", "graph_view_expand", "graph_view_search", "graph_view_labels",
                     "graph_view_upload", "graph_view_edit"):
            assert tools[name].meta["ui"]["visibility"] == ["app"], name
        # Claude's own tools: no UI of their own, visible to the model.
        assert "ui" not in (tools["graph_view_update"].meta or {})

        contents = await s.c.read_resource("ui://rete/graph-view.html")
        assert contents[0].mime_type == "text/html;profile=mcp-app"
        assert contents[0].meta["ui"] == {"prefersBorder": True}
        page = contents[0].text
        assert page.startswith("<!doctype html>")
        assert "cytoscape" in page and "<script>" in page
        assert "<script src=" not in page and "<script type=\"module\" src=" not in page, \
            "everything inline: the sandbox has no network"

    run(t)


def test_a_view_opens_on_a_query_syncs_takes_claudes_changes_and_reports_the_users():
    async def t(s):
        opened = await s.call("graph_view", {"dataset": PEOPLE, "query": EDGES, "title": "who knows whom"})
        sc = opened.structured_content
        view_id = sc["view_id"]
        assert view_id[0] == "v" and view_id[1:].isdigit() and len(view_id) == 13
        assert (sc["nodes"], sc["edges"]) == (3, 3)
        assert "graph_view_update" in opened.content[0].text

        # The page starts from nothing: it gets the whole graph.
        first = await s.data("graph_view_sync", {"view_id": view_id, "since": 0})
        assert len(first["snapshot"]["nodes"]) == 3 and len(first["snapshot"]["edges"]) == 3
        assert all(n["iri"].startswith("https://example.org/") for n in first["snapshot"]["nodes"])

        # Labels and types, asked for by the page.
        ids = [n["id"] for n in first["snapshot"]["nodes"]]
        labelled = await s.data("graph_view_labels", {"view_id": view_id, "ids": ids, "since": first["version"]})
        labels = next(p for p in labelled["patches"] if p["op"] == "labels")["labels"]
        assert labels[P("ada")] == {"label": "Ada Lovelace", "types": ["https://example.org/Person"]}

        # Claude changes the look and adds a query's results.
        update = await s.data("graph_view_update", {
            "view_id": view_id,
            "options": {"group_by": "type", "edge_labels": True, "layout": "circle"},
            "query": "PREFIX ex: <https://example.org/> CONSTRUCT { ?p ex:worksFor ?o . ?o a ex:Project } "
                     "WHERE { ?p ex:worksFor ?o }",
        })
        assert update["nodes"] == 4
        assert "added 1 nodes" in " ".join(update["done"])
        after = await s.data("graph_view_sync", {"view_id": view_id, "since": labelled["version"]})
        assert [p["op"] for p in after["patches"]] == ["options", "add"]
        assert after["patches"][0]["options"]["layout"] == "circle"

        # The user selects something; Claude reads it.
        await s.data("graph_view_sync", {"view_id": view_id, "since": after["version"], "state": {
            "selection": [{"id": P("ada"), "iri": "https://example.org/ada", "label": "Ada Lovelace"}],
            "selection_count": 1}})
        st = await s.data("graph_view_state", {"view_id": view_id})
        assert st["user"]["open"] is True
        assert st["user"]["selection"][0]["label"] == "Ada Lovelace"

        # Expanding a node brings both directions.
        grown = await s.data("graph_view_expand", {"view_id": view_id, "iri": "https://example.org/engine",
                                                   "since": after["version"]})
        added = next(p for p in grown["patches"] if p["op"] == "add" and p.get("focus"))
        assert any(n["id"] == P("lab") for n in added["nodes"]), "the incoming ex:hosts edge brings ex:lab"
        assert added["focus"] == P("engine")

        # A page far behind (or new) gets everything again.
        whole = await s.data("graph_view_sync", {"view_id": view_id, "since": 0})
        assert len(whole["snapshot"]["nodes"]) >= 5
        assert whole["snapshot"]["options"]["layout"] == "circle"

    run(t)


def test_select_rows_become_nodes_links_and_properties_search_adds_entities():
    async def t(s):
        opened = await s.call("graph_view", {"dataset": PEOPLE, "query": (
            "PREFIX ex: <https://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
            "SELECT ?person ?name ?org WHERE { ?person rdfs:label ?name ; ex:worksFor ?org }")})
        view_id = opened.structured_content["view_id"]
        snap = (await s.data("graph_view_sync", {"view_id": view_id, "since": 0}))["snapshot"]
        ada = next(n for n in snap["nodes"] if n["id"] == P("ada"))
        assert ada["label"] == "Ada Lovelace", "a ?name column names the row's entity"
        assert ["?name", "Ada Lovelace"] in ada["props"]
        assert any(e["label"] == "?org" and e["target"] == P("engine") for e in snap["edges"])
        assert snap["table"]["vars"] == ["person", "name", "org"]
        assert len(snap["table"]["rows"]) == 2

        found = await s.data("graph_view_search", {"view_id": view_id, "text": "Mary", "since": 0})
        assert found["found"] == 1

        # An entity's neighbourhood as the starting point.
        around = await s.call("graph_view", {"dataset": PEOPLE, "iri": "https://example.org/charles"})
        assert around.structured_content["nodes"] >= 3

    run(t)


def test_a_file_attached_to_the_chat_is_handed_over_in_a_view():
    async def t(s):
        path = "/mnt/user-data/uploads/talks.rete"
        # Every other tool says how to get it here.
        why = await s.refusal("sparql_query", {"dataset": path, "query": "ASK { ?s ?p ?o }"})
        assert "attached to the chat: Claude's copy of it is in Claude's own sandbox" in why
        assert f'call graph_view with dataset "{path}"' in why

        # graph_view opens a view that asks the user for it…
        waiting = await s.call("graph_view", {"dataset": path, "query": EDGES})
        view_id = waiting.structured_content["view_id"]
        assert waiting.structured_content["awaiting"] == "talks.rete"
        assert "waiting for talks.rete" in waiting.content[0].text
        first = await s.data("graph_view_sync", {"view_id": view_id, "since": 0})
        assert first["snapshot"]["awaiting"] == {"file": "talks.rete"}
        assert "still waiting for the user to open talks.rete" in await s.refusal(
            "graph_view_update", {"view_id": view_id, "query": EDGES})

        # …and the page sends it in parts.
        data = PEOPLE_BYTES
        cut = len(data) // 2

        def part(start, end):
            return {"view_id": view_id, "name": "talks.rete", "size": len(data), "offset": start,
                    "data": base64.b64encode(data[start:end]).decode(), "since": first["version"]}

        assert "out of order" in await s.refusal("graph_view_upload", part(cut, len(data)))
        assert await s.data("graph_view_upload", part(0, cut)) == {"received": cut, "size": len(data)}
        done = await s.data("graph_view_upload", part(cut, len(data)))
        assert done["dataset"].startswith("generated/") and done["dataset"].endswith(".rete")
        assert done["statements"] == 17
        assert done["awaited"] is True
        # The view had shown nothing yet, so it gets the whole of it: what Claude asked for.
        assert done["snapshot"]["dataset"] == done["dataset"]
        assert done["snapshot"]["awaiting"] is None
        assert len(done["snapshot"]["nodes"]) == 3

        # The file is a dataset here now, for every tool.
        count = await s.data("sparql_query", {"dataset": done["dataset"],
                                               "query": "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"})
        assert count["table"] == [{"n": 17}]
        st = await s.data("graph_view_state", {"view_id": view_id})
        assert st["dataset"] == done["dataset"] and "awaiting" not in st

    run(t)


def test_a_file_the_user_opens_in_a_view_replaces_what_it_shows():
    async def t(s):
        view_id = (await s.call("graph_view", {"dataset": PEOPLE, "query": EDGES})).structured_content["view_id"]
        ttl = b"@prefix ex: <https://example.org/> .\nex:x ex:p ex:y .\nex:y ex:p ex:z .\n"
        done = await s.data("graph_view_upload", {"view_id": view_id, "name": "extra.ttl", "size": len(ttl),
                                                  "offset": 0, "data": base64.b64encode(ttl).decode(), "since": 0})
        assert done["dataset"].startswith("generated/")
        assert done["awaited"] is False
        assert done["snapshot"]["dataset"] == done["dataset"]
        assert done["snapshot"]["title"] == "extra.ttl"
        assert len(done["snapshot"]["nodes"]) == 3, "a sample of the new file, nothing of the old one"

        junk = b"not a graph at all"
        why = await s.refusal("graph_view_upload", {"view_id": view_id, "name": "junk.rete", "size": len(junk),
                                                    "offset": 0, "data": base64.b64encode(junk).decode(),
                                                    "since": 0})
        assert "junk.rete is not a .rete file this server can read" in why

    run(t)


def test_uploads_are_capped_and_off_without_authoring():
    async def t(s):
        view_id = (await s.call("graph_view", {"dataset": PEOPLE, "query": EDGES})).structured_content["view_id"]
        why = await s.refusal("graph_view_upload", {"view_id": view_id, "name": "big.rete", "size": 2 << 20,
                                                    "offset": 0, "data": "", "since": 0})
        assert "more than the 1.0 MB a file opened in a graph view may be" in why

    run(t, upload_max_mb=1)

    async def off(s):
        why = await s.refusal("graph_view", {"dataset": "/mnt/user-data/uploads/talks.rete"})
        assert "does not take files from a graph view (authoring is off)" in why
        view_id = (await s.call("graph_view", {"dataset": PEOPLE, "query": EDGES})).structured_content["view_id"]
        why = await s.refusal("graph_view_upload", {"view_id": view_id, "name": "x.ttl", "size": 3, "offset": 0,
                                                    "data": base64.b64encode(b"abc").decode(), "since": 0})
        assert "authoring is off" in why

    run(off, author=False)


def test_a_diagram_comes_with_its_look_a_schema_view_draws_classes_with_their_properties():
    async def t(s):
        view_id = (await s.call("graph_view", {"dataset": PEOPLE, "query": EDGES})).structured_content["view_id"]
        first = await s.data("graph_view_sync", {"view_id": view_id, "since": 0})

        # Boxes and classes come in layers with square lines…
        await s.data("graph_view_update", {"view_id": view_id, "options": {"diagram": "classes"}})
        classes = await s.data("graph_view_sync", {"view_id": view_id, "since": first["version"]})
        assert classes["patches"][0]["options"] == {"diagram": "classes", "layout": "layered",
                                                    "edge_style": "square"}
        # …unless the call says otherwise.
        await s.data("graph_view_update", {"view_id": view_id, "options": {"diagram": "graph",
                                                                           "edge_style": "square"}})
        dots = await s.data("graph_view_sync", {"view_id": view_id, "since": classes["version"]})
        assert dots["patches"][0]["options"]["layout"] == "cose"
        assert dots["patches"][0]["options"]["edge_style"] == "square"

        # The dataset's own schema, with no query.
        schema = await s.call("graph_view", {"dataset": PEOPLE, "schema": True})
        assert "from its schema pyramid): 3 classes" in schema.content[0].text
        snap = (await s.data("graph_view_sync", {"view_id": schema.structured_content["view_id"],
                                                 "since": 0}))["snapshot"]
        assert snap["title"] == f"schema of {PEOPLE}"
        assert snap["options"]["diagram"] == "boxes" and snap["options"]["layout"] == "layered"
        person = next(n for n in snap["nodes"] if n["iri"] == "https://example.org/Person")
        assert person["kind"] == "class" and person["count"] == 3
        assert ["http://www.w3.org/2000/01/rdf-schema#label", "3"] in person["props"]
        assert any(e["source"] == P("Person") and e["target"] == P("Project") and e["label"] == "worksFor (2)"
                   for e in snap["edges"]), snap["edges"]
        assert snap["table"]["vars"] == ["class", "property", "object", "count"]

    run(t)


def test_the_users_edits_and_claudes_aggregates_by_type():
    async def t(s):
        everything = "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o FILTER(isIRI(?o)) }"
        view_id = (await s.call("graph_view", {"dataset": PEOPLE, "query": everything})).structured_content["view_id"]
        first = await s.data("graph_view_sync", {"view_id": view_id, "since": 0})
        assert len(first["snapshot"]["nodes"]) == 5

        # The user aggregates the three people…
        made = await s.data("graph_view_edit", {"view_id": view_id, "since": first["version"], "aggregate": {
            "ids": [P("ada"), P("charles"), P("mary")], "label": "Person"}})
        agg = next(p for p in made["patches"] if p["op"] == "aggregate")["aggregate"]
        assert {**agg, "members": sorted(agg["members"])} == {
            "id": "agg:1", "label": "Person", "members": [P("ada"), P("charles"), P("mary")]}
        # …which Claude sees…
        assert (await s.data("graph_view_state", {"view_id": view_id}))["aggregates"] == [
            {"id": "agg:1", "label": "Person", "members": 3}]
        # …a removal takes a member out of it…
        removed = await s.data("graph_view_edit", {"view_id": view_id, "remove": [P("mary")],
                                                   "since": made["version"]})
        assert [p["op"] for p in removed["patches"]] == ["remove"]
        after = await s.data("graph_view_state", {"view_id": view_id})
        assert after["nodes"] == 4 and after["aggregates"][0]["members"] == 2
        # …and it is split up again.
        split = await s.data("graph_view_edit", {"view_id": view_id, "ungroup": "agg:1",
                                                 "since": removed["version"]})
        assert split["patches"][-1] == {"version": split["version"], "op": "ungroup", "ids": ["agg:1"]}

        # Claude aggregates every node of a type, then ungroups them all.
        by_type = await s.data("graph_view_update", {"view_id": view_id,
                                                     "aggregate": {"type": "https://example.org/Person"}})
        assert 'aggregated 2 nodes as agg:2 "Person"' in " ".join(by_type["done"])
        everyone = await s.data("graph_view_update", {"view_id": view_id, "ungroup": "all"})
        assert "ungrouped 1 aggregate" in " ".join(everyone["done"])

        assert "at least two" in await s.refusal("graph_view_edit", {"view_id": view_id, "since": 0,
                                                                     "aggregate": {"ids": [P("ada")]}})

    run(t)


def test_bad_calls_say_what_is_wrong():
    async def t(s):
        assert "no graph view v999" in await s.refusal("graph_view_update", {"view_id": "v999",
                                                                             "options": {"labels": False}})
        assert "no graph to show" in await s.refusal("graph_view", {"dataset": PEOPLE,
                                                                    "query": "ASK { ?s ?p ?o }"})
        assert "unknown dataset" in await s.refusal("graph_view", {"dataset": "nope", "query": EDGES})

    run(t)


@pytest.mark.parametrize("since", [0, 1])
def test_a_view_far_behind_gets_a_snapshot(since):
    """Only the last MAX_LOG patches are kept: a page behind them starts over."""
    from rete_server import views as v

    view = v.View("v1", PEOPLE, "t", {})
    for _ in range(v.MAX_LOG + 5):
        view.patch({"op": "status", "status": ""})
    assert "snapshot" in view.since(since)
    assert "patches" in view.since(view.version - 3)
