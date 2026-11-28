"""Skills over MCP (SEP-2640, extension ``io.modelcontextprotocol/skills``).

Four skills ship in ``rete_server/skills/``: ``build-rete``, ``query``,
``network-analysis`` and ``web-pages``. Each is a folder with a ``SKILL.md``
(YAML frontmatter with ``name`` and ``description``, then instructions) and
optional files under ``references/``. A client that supports the extension
lists them with ``skills/list``, shows only name and description, and loads
``SKILL.md`` and the other files when the skill is used. Any client can also
read the files as ``skill://<name>/…`` resources.

The folder is read once at startup (an invalid skill stops the server with a
message that says what to fix), so a deployment serves the skills it was built
with. FastMCP registers the extension by itself when a skills provider is
added. It answers ``skills/list`` and ``skills/get`` on protocol 2026-07-28;
older connections still read the same files as resources.
"""
from __future__ import annotations

from pathlib import Path

from fastmcp import FastMCP
from fastmcp.skills.provider import SkillsDirectoryProvider

SKILLS_DIR = Path(__file__).resolve().parent / "skills"


def register_skills(mcp: FastMCP) -> SkillsDirectoryProvider:
    """Serve the skills under ``rete_server/skills/`` from ``mcp``."""
    provider = SkillsDirectoryProvider(SKILLS_DIR)
    mcp.add_provider(provider)
    return provider
