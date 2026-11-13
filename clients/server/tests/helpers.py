"""Small helpers shared by the tests."""
from __future__ import annotations

import json
from typing import Any


def tool_json(result: Any) -> Any:
    """The JSON a FastMCP tool returned (a CallToolResult or bare content list)."""
    content = result.content if hasattr(result, "content") else result
    return json.loads(content[0].text)
