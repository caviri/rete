"""Natural-language → SPARQL, via a pydantic-ai agent (env-gated).

Only imported when ASK_MODEL is set (e.g. ``anthropic:claude-sonnet-5`` with
ANTHROPIC_API_KEY, or ``openai:gpt-5-mini`` with OPENAI_API_KEY). The agent
gets the same self-describing surface the MCP tools expose — schema, card,
examples, entity search, query — scoped to one dataset per request.
"""
from __future__ import annotations

import json
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field
from pydantic_ai import Agent, RunContext

import rete_service as svc


class Answer(BaseModel):
    answer: str = Field(description="A direct answer to the question, grounded in the query results")
    sparql: str = Field(description="The final SPARQL query that produced the evidence")
    table: List[Dict[str, Any]] = Field(default_factory=list, description="Result rows backing the answer")


_SYSTEM = """\
You answer questions about ONE RDF knowledge graph by writing and running
SPARQL. Ground every answer in query results — never guess.

Method: read the schema first; copy class/predicate IRIs exactly (no invented
namespaces). If the question names a specific thing, resolve it with
find_entities before querying. Look at example_queries for the graph's idioms.
Iterate: run a query, inspect rows, refine. Always use LIMIT (≤ 200).
"""


def answer_question(model: str, dataset: str, question: str) -> Dict[str, Any]:
    agent: Agent[str, Answer] = Agent(model, output_type=Answer, system_prompt=_SYSTEM)

    @agent.tool
    def dataset_schema(ctx: RunContext[str]) -> str:
        """Classes with instance counts + subject-class/predicate/object-class relations."""
        return json.dumps(svc.dataset_schema(ctx.deps))

    @agent.tool
    def dataset_card(ctx: RunContext[str]) -> str:
        """The dataset's embedded card (description, license, counts)."""
        return json.dumps(svc.dataset_card(ctx.deps) or {})

    @agent.tool
    def example_queries(ctx: RunContext[str]) -> str:
        """Runnable example SPARQL queries the dataset ships with."""
        return json.dumps(svc.dataset_examples(ctx.deps))

    @agent.tool
    def find_entities(ctx: RunContext[str], text: str) -> str:
        """Resolve a name to entity IRIs (label prefix + text index)."""
        return json.dumps(svc.entity_search(text, ctx.deps, 20))

    @agent.tool
    def sparql_query(ctx: RunContext[str], query: str) -> str:
        """Run SPARQL against the dataset; returns rows as plain values."""
        doc = svc.run_query(ctx.deps, None, query, False, 200)
        return json.dumps({k: doc.get(k) for k in ("kind", "boolean", "table", "triples", "truncated")})

    result = agent.run_sync(question, deps=dataset)
    out = result.output
    return {"dataset": dataset, "question": question, "answer": out.answer,
            "sparql": out.sparql, "table": out.table}
