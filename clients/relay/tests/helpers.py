"""Small helpers shared by the relay tests."""
from __future__ import annotations

import json
from typing import Any


def tool_json(result: Any) -> Any:
    """The JSON a FastMCP tool returned, across fastmcp 2.x result shapes
    (a CallToolResult with `.content`, or a bare list of content blocks)."""
    content = result.content if hasattr(result, "content") else result
    return json.loads(content[0].text)
