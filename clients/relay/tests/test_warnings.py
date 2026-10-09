"""FILTER type-error warnings (#279-#283) reach every caller of relay.

`CONTAINS(?s, "alice")` on an IRI is a SPARQL type error; FILTER treats it as
false, so the query returns zero rows and, before rete-graph 0.3.3, no reason.
The engine now reports it, and `Graph.last_warnings()` exposes it. These tests
pin that relay hands it on: REST, the SPARQL 1.1 Protocol endpoint, the MCP
tool, and the /api/ask agent's query tool. No LLM is called: the agent runs on
pydantic-ai's FunctionModel, a scripted stand-in.
"""
from __future__ import annotations

import asyncio
import json
from urllib.parse import quote

from conftest import CONTAINS_ON_IRI, NAME, PEOPLE
from helpers import tool_json


def _assert_contains_on_iri(warnings):
    assert len(warnings) == 1, warnings
    w = warnings[0]
    assert (w["function"], w["argument"], w["argKind"]) == ("CONTAINS", 1, "iri")
    assert "STR()" in w["hint"] and w["message"]


def test_rest_returns_the_warning_with_the_empty_result(client):
    r = client.post("/api/query", json={"dataset": PEOPLE, "query": CONTAINS_ON_IRI})
    assert r.status_code == 200, r.text
    doc = r.json()
    assert doc["table"] == []  # results unchanged: the spec says zero rows
    _assert_contains_on_iri(doc["warnings"])

    fixed = CONTAINS_ON_IRI.replace("CONTAINS(?s,", "CONTAINS(STR(?s),")
    doc = client.get(f"/api/query?dataset={quote(PEOPLE)}&q={quote(fixed)}").json()
    assert len(doc["table"]) == 1 and doc["warnings"] == []


def test_protocol_carries_the_warning_in_a_header_and_the_body(client):
    r = client.get(f"/sparql/{PEOPLE}?query={quote(CONTAINS_ON_IRI)}")
    assert r.status_code == 200
    body = r.json()
    assert body["results"]["bindings"] == []
    _assert_contains_on_iri(body["warnings"])
    (message,) = json.loads(r.headers["x-rete-warnings"])
    assert message == body["warnings"][0]["message"]


def test_protocol_construct_and_ask_carry_the_header(client):
    construct = (f'CONSTRUCT {{ ?s {NAME} ?n }} WHERE {{ ?s {NAME} ?n '
                 'FILTER(CONTAINS(?s, "alice")) }')
    r = client.get(f"/sparql/{PEOPLE}?query={quote(construct)}")
    assert r.text == "" and "CONTAINS" in r.headers["x-rete-warnings"]

    ask = f'ASK {{ ?s {NAME} ?n FILTER(STRSTARTS(?s, "http")) }}'
    r = client.get(f"/sparql/{PEOPLE}?query={quote(ask)}")
    assert r.json()["boolean"] is False
    assert r.json()["warnings"][0]["function"] == "STRSTARTS"
    assert "STRSTARTS" in r.headers["x-rete-warnings"]


def test_mcp_sparql_query_tool_returns_the_warning():
    from fastmcp import Client

    import rete_mcp

    async def go():
        async with Client(rete_mcp.mcp) as c:
            return tool_json(await c.call_tool(
                "sparql_query", {"dataset": PEOPLE, "query": CONTAINS_ON_IRI}))

    doc = asyncio.run(go())
    assert doc["table"] == []
    _assert_contains_on_iri(doc["warnings"])


def test_mcp_instructions_tell_the_agent_about_str():
    import rete_mcp

    assert "STR()" in rete_mcp.INSTRUCTIONS and "`warnings`" in rete_mcp.INSTRUCTIONS


def test_ask_agent_sees_the_warning_in_its_query_tool():
    """Script the model: call sparql_query with the CONTAINS-on-IRI query,
    then finish. Capture what the tool handed back to the 'model'."""
    from pydantic_ai.messages import ModelResponse, ToolCallPart, ToolReturnPart
    from pydantic_ai.models.function import AgentInfo, FunctionModel

    import rete_ask

    seen = []

    def script(messages, info: AgentInfo) -> ModelResponse:
        returns = [p for m in messages for p in getattr(m, "parts", [])
                   if isinstance(p, ToolReturnPart) and p.tool_name == "sparql_query"]
        if not returns:
            return ModelResponse(parts=[ToolCallPart("sparql_query", {"query": CONTAINS_ON_IRI})])
        seen.append(json.loads(returns[-1].content))
        final = info.output_tools[0].name
        return ModelResponse(parts=[ToolCallPart(final, {
            "answer": "No rows; CONTAINS needs STR() on an IRI.",
            "sparql": CONTAINS_ON_IRI, "table": []})])

    out = rete_ask.answer_question(FunctionModel(script), PEOPLE, "Whose IRI contains alice?")
    assert out["sparql"] == CONTAINS_ON_IRI
    (tool_doc,) = seen
    assert tool_doc["table"] == []
    _assert_contains_on_iri(tool_doc["warnings"])


def test_ask_endpoint_is_off_without_a_model(client):
    r = client.post("/api/ask", json={"dataset": PEOPLE, "question": "anything"})
    assert r.status_code == 503
