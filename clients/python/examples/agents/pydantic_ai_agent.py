"""A Pydantic AI agent that answers questions from one `.rete` graph.

    pip install rete-graph "pydantic-ai-slim[anthropic]"
    export ANTHROPIC_API_KEY=...
    python pydantic_ai_agent.py "How many laws are in this graph, and which are the oldest?"

Defaults to the published Spanish-legislation graph (7 MB on a CDN, read by
byte range). Point it anywhere with RETE_GRAPH=/path/to/your.rete.

Any OpenAI-compatible endpoint works instead of Anthropic:

    export OPENAI_BASE_URL=https://your-endpoint/v1 OPENAI_API_KEY=... MODEL=...
"""

from __future__ import annotations

import asyncio
import os
import sys

from pydantic_ai import Agent

from rete_tools import SYSTEM_PROMPT, ReteGraphTools

GRAPH = os.environ.get("RETE_GRAPH", "https://data.graphplaza.com/boe/boe.rete")


def build_model():
    """An Anthropic model by default; an OpenAI-compatible endpoint if configured."""
    model = os.environ.get("MODEL", "anthropic:claude-sonnet-5")
    base_url = os.environ.get("OPENAI_BASE_URL")
    if not base_url:
        return model

    from pydantic_ai.models.openai import OpenAIChatModel
    from pydantic_ai.providers.openai import OpenAIProvider

    return OpenAIChatModel(
        model,
        provider=OpenAIProvider(base_url=base_url, api_key=os.environ["OPENAI_API_KEY"]),
    )


async def main(question: str) -> None:
    tools = ReteGraphTools(GRAPH)

    # Bound methods: pydantic-ai reads the signature for the schema and the
    # docstring for the description, so the tools need no extra declaration.
    agent = Agent(
        build_model(),
        system_prompt=SYSTEM_PROMPT,
        tools=[
            tools.dataset_card,
            tools.dataset_schema,
            tools.example_queries,
            tools.find_entities,
            tools.sparql_query,
        ],
    )

    result = await agent.run(question)
    print(result.output)
    print(f"\n[graph reads: {tools.read_stats()}]")


if __name__ == "__main__":
    question = " ".join(sys.argv[1:]) or "What is in this graph? Show me five example entities."
    asyncio.run(main(question))
