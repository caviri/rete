"""A LangChain agent that answers questions from one `.rete` graph.

    pip install rete-graph langchain langchain-anthropic
    export ANTHROPIC_API_KEY=...
    python langchain_agent.py "How many laws are in this graph, and which are the oldest?"

Defaults to the published Spanish-legislation graph (7 MB on a CDN, read by
byte range). Point it anywhere with RETE_GRAPH=/path/to/your.rete.

Any OpenAI-compatible endpoint works instead of Anthropic:

    pip install langchain-openai
    export OPENAI_BASE_URL=https://your-endpoint/v1 OPENAI_API_KEY=... MODEL=...
"""

from __future__ import annotations

import os
import sys

from langchain.agents import create_agent
from langchain_core.tools import StructuredTool

from rete_tools import SYSTEM_PROMPT, ReteGraphTools

GRAPH = os.environ.get("RETE_GRAPH", "https://data.graphplaza.com/boe/boe.rete")


def build_model():
    """An Anthropic model by default; an OpenAI-compatible endpoint if configured."""
    model = os.environ.get("MODEL", "claude-sonnet-5")
    base_url = os.environ.get("OPENAI_BASE_URL")
    if not base_url:
        from langchain.chat_models import init_chat_model

        return init_chat_model(model, model_provider="anthropic")

    from langchain_openai import ChatOpenAI

    return ChatOpenAI(model=model, base_url=base_url, api_key=os.environ["OPENAI_API_KEY"])


def rete_toolkit(source: str) -> tuple[ReteGraphTools, list[StructuredTool]]:
    """Bound methods → LangChain tools; name and description come from the method."""
    tools = ReteGraphTools(source)
    methods = [
        tools.dataset_card,
        tools.dataset_schema,
        tools.example_queries,
        tools.find_entities,
        tools.sparql_query,
    ]
    return tools, [StructuredTool.from_function(m, name=m.__name__) for m in methods]


def main(question: str) -> None:
    tools, toolkit = rete_toolkit(GRAPH)
    agent = create_agent(build_model(), toolkit, system_prompt=SYSTEM_PROMPT)

    result = agent.invoke({"messages": [{"role": "user", "content": question}]})
    print(result["messages"][-1].content)
    print(f"\n[graph reads: {tools.read_stats()}]")


if __name__ == "__main__":
    main(" ".join(sys.argv[1:]) or "What is in this graph? Show me five example entities.")
