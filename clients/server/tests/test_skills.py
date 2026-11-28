"""Skills over MCP (SEP-2640): the four skills the server ships.

Protocol level: ``skills/list`` and ``skills/get`` on 2026-07-28, the files as
``skill://`` resources on both eras, and the old 2025-11-25 handshake still
working with every tool. Content level: every tool and argument the skills name
exists, every SPARQL block parses and runs on a real graph, the worked example
builds and verifies as the skill says it does.
"""
from __future__ import annotations

import ast
import asyncio
import hashlib
import json
import pathlib
import re

import pytest
import rete_graph as rete
import yaml

from conftest import DATA, serve
from helpers import tool_json

SKILLS = pathlib.Path(__file__).resolve().parents[1] / "rete_server" / "skills"
NAMES = ["build-rete", "network-analysis", "query", "web-pages"]
FIXTURE = pathlib.Path(__file__).with_name("skills_fixture.ttl")
(DATA / "skills-fixture.rete").write_bytes(rete.build(FIXTURE.read_text(encoding="utf-8"), "ttl"))
KEY = "local/skills-fixture.rete"

TEXT_SUFFIXES = {".md", ".html", ".mjs", ".js", ".json", ".txt", ".ttl"}
MAX_FILE = 100 * 1024     # Claude Code: 100 KB per text file
MAX_SKILL = 400 * 1024    # and 400 KB per skill
PREFIXES = {
    "ex": "https://example.org/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "rdf": "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "xsd": "http://www.w3.org/2001/XMLSchema#",
    "schema": "https://schema.org/",
}


def files_of(name):
    root = SKILLS / name
    return sorted(p for p in root.rglob("*") if p.is_file())


def all_markdown():
    return [p for n in NAMES for p in files_of(n) if p.suffix == ".md"]


def blocks(lang):
    """Fenced blocks of one language, across every skill file: (path, text)."""
    found = []
    for p in all_markdown():
        for m in re.finditer(r"```" + lang + r"\n(.*?)```", p.read_text(encoding="utf-8"), re.S):
            found.append((p, m.group(1)))
    return found


def with_prefixes(query):
    """The examples declare no prefixes; add the ones the query uses."""
    lines = [f"PREFIX {k}: <{v}>" for k, v in PREFIXES.items()
             if re.search(rf"(?<![\w<]){k}:", query) and not re.search(rf"PREFIX\s+{k}:", query, re.I)]
    return "\n".join(lines + [query])


def run(coro_fn, mode="auto", author=True, **overrides):
    """Run ``coro_fn(client)`` against a fresh server. ``auto`` negotiates 2026-07-28."""
    from fastmcp import Client
    from fastmcp.skills import SkillsClientExtension

    from rete_server.mcp import build_mcp

    async def go():
        kw = {"mode": mode}
        if mode != "legacy":
            kw["extensions"] = [SkillsClientExtension()]
        async with Client(build_mcp(author=author), **kw) as c:
            return await coro_fn(c)

    with serve(**overrides):
        return asyncio.run(go())


def sha(path):
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


# --- the folders ---------------------------------------------------------------

def test_the_four_skills_are_there():
    assert sorted(p.name for p in SKILLS.iterdir() if p.is_dir()) == NAMES


@pytest.mark.parametrize("name", NAMES)
def test_frontmatter_names_the_folder_and_says_when_to_use_it(name):
    text = (SKILLS / name / "SKILL.md").read_bytes().decode()
    assert text.startswith("---\n")
    meta = yaml.safe_load(text.split("---\n", 2)[1])
    assert meta["name"] == name
    description = meta["description"]
    assert 100 < len(description) <= 1024
    assert "Use when" in description, "the description is all a host shows: it must say when to use the skill"


@pytest.mark.parametrize("name", NAMES)
def test_sizes_stay_inside_claude_codes_limits(name):
    files = files_of(name)
    assert sum(p.stat().st_size for p in files) <= MAX_SKILL
    for p in files:
        assert p.suffix in TEXT_SUFFIXES, f"{p} is not a text file"
        assert p.stat().st_size <= MAX_FILE
    skill_md = (SKILLS / name / "SKILL.md").stat().st_size
    assert 4_000 <= skill_md <= 12_500, f"{name}/SKILL.md is {skill_md} bytes"


def test_instructions_stay_under_claude_codes_truncation():
    from rete_server.mcp import build_mcp

    for author in (True, False):
        text = build_mcp(author=author).instructions
        assert len(text) < 3_500, len(text)   # Claude Code cuts at 4,096
        for name in NAMES:
            assert f"`{name}`" in text
    # the workflow for clients without skills is still there
    assert "`dataset_schema(dataset)`" in text and "STR()" in text


def test_relative_links_resolve():
    for p in all_markdown():
        for target in re.findall(r"\]\((?!https?:|#)([^)#]+)", p.read_text(encoding="utf-8")):
            assert (p.parent / target).is_file(), f"{p.relative_to(SKILLS)} links to {target}"


# --- the protocol ---------------------------------------------------------------

def test_skills_list_on_2026_07_28_matches_the_files():
    async def t(c):
        assert c.protocol_version == "2026-07-28"
        return await c.list_skills()

    skills = run(t)
    assert sorted(s.frontmatter.name for s in skills) == NAMES
    for s in skills:
        name = s.frontmatter.name
        assert s.uri == f"skill://{name}/SKILL.md"
        by_uri = {r.uri: r for r in s.resources}
        files = files_of(name)
        assert len(by_uri) == len(files)
        for p in files:
            rel = p.relative_to(SKILLS / name).as_posix()
            r = by_uri[f"skill://{name}/{rel}"]
            assert r.digest == sha(p) and r.size == p.stat().st_size, rel
        meta = yaml.safe_load((SKILLS / name / "SKILL.md").read_text(encoding="utf-8").split("---\n", 2)[1])
        assert s.frontmatter.description == meta["description"]


def test_skills_get_and_resource_read_return_the_file():
    async def t(c):
        out = {}
        for name in NAMES:
            uri = f"skill://{name}/SKILL.md"
            got = await c.get_skill_mcp(uri)
            read = await c.read_resource(uri)
            out[name] = (got.skill, read[0].text)
        return out

    for name, (skill, text) in run(t).items():
        assert skill.uri == f"skill://{name}/SKILL.md"
        assert text == (SKILLS / name / "SKILL.md").read_bytes().decode()


def test_supporting_files_read_as_resources():
    async def t(c):
        page = await c.read_resource("skill://web-pages/references/page.html")
        card = await c.read_resource("skill://build-rete/references/card.md")
        return page[0].text, card[0].text

    page, card = run(t)
    assert page == (SKILLS / "web-pages/references/page.html").read_text(encoding="utf-8")
    assert card == (SKILLS / "build-rete/references/card.md").read_text(encoding="utf-8")


def test_an_unknown_skill_is_refused():
    from mcp.shared.exceptions import MCPError

    async def t(c):
        with pytest.raises(MCPError):
            await c.get_skill_mcp("skill://nope/SKILL.md")

    run(t)


def test_the_old_handshake_still_works_with_every_tool():
    async def t(c):
        assert c.protocol_version == "2025-11-25"
        tools = {x.name for x in await c.list_tools()}
        listed = tool_json(await c.call_tool("list_datasets", {}))
        resources = {str(r.uri) for r in await c.list_resources()}
        text = (await c.read_resource("skill://query/SKILL.md"))[0].text
        return tools, listed, resources, text

    tools, listed, resources, text = run(t, mode="legacy")
    assert {"list_datasets", "dataset_card", "dataset_schema", "example_queries", "sparql_query",
            "validate_query", "find_entities", "describe_entity", "validate_shacl", "shacl_shapes",
            "search", "fetch", "graph_view", "graph_view_update", "graph_view_state",
            "suggest_vocabulary", "check_ontology", "build_rete", "causal_diagram"} <= tools
    assert any(d["key"] == "people" for d in listed)
    assert {f"skill://{n}/SKILL.md" for n in NAMES} <= resources
    assert text == (SKILLS / "query/SKILL.md").read_text(encoding="utf-8")


def test_the_2026_connection_has_the_same_tools():
    async def t(c):
        return {x.name for x in await c.list_tools()}

    assert run(t) == run(t, mode="legacy")


# --- what the skills say about the tools ----------------------------------------

def _property_names(schema):
    names = set()
    if isinstance(schema, dict):
        names.update((schema.get("properties") or {}).keys())
        for v in schema.values():
            names |= _property_names(v)
    elif isinstance(schema, list):
        for v in schema:
            names |= _property_names(v)
    return names


def _top_level_args(call):
    """``a, b=[x, y], c`` -> ['a', 'b', 'c'] (commas inside brackets do not split)."""
    depth, cur, out = 0, "", []
    for ch in call:
        depth += ch in "([{"
        depth -= ch in ")]}"
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    out.append(cur)
    return [re.match(r"\s*([A-Za-z_]\w*)", a).group(1) for a in out if re.match(r"\s*([A-Za-z_]\w*)\s*(=|$)", a)]


# snake_case words in backticks that are neither a server tool nor a tool argument
RESULT_FIELDS = {"content_hash", "triple_count", "term_count", "elapsed_seconds"}      # in answers
CARD_KEYS = {"source_date", "cite_as", "canonical_url", "sparql_endpoint", "derived_from"}  # in `card`
PAGE_TOOLS = {"run_sparql", "search_papers", "get_paper", "get_dataset_info", "get_example_queries"}  # WebMCP, on a page
NOT_TOOLS = RESULT_FIELDS | CARD_KEYS | PAGE_TOOLS


def test_every_tool_and_argument_the_skills_name_exists():
    async def t(c):
        return {x.name: x.input_schema for x in await c.list_tools()}

    tools = run(t)
    params = set()
    for schema in tools.values():
        params |= _property_names(schema)
    problems = []
    for p in all_markdown():
        text = p.read_text(encoding="utf-8")
        text = re.sub(r"```.*?```", "", text, flags=re.S)       # prose only; code blocks have their own checks
        for token in re.findall(r"`([^`\n]+)`", text):
            m = re.fullmatch(r"([a-z][a-z0-9]*(?:_[a-z0-9]+)+)(?:\((.*)\))?(?:=\S*)?", token)
            if not m:
                continue
            word, call = m.group(1), m.group(2)
            if word in tools:
                for arg in _top_level_args(call or ""):
                    if arg not in tools[word].get("properties", {}) and arg not in params:
                        problems.append(f"{p.name}: {word}({arg}) is not an argument")
            elif word not in params and word not in NOT_TOOLS:
                problems.append(f"{p.name}: `{token}` is neither a tool nor an argument")
    assert not problems, "\n".join(problems)


def test_tools_named_in_prose_exist_for_the_authoring_split():
    # The skills tell the reader that the authoring tools may be missing.
    async def t(c):
        return {x.name for x in await c.list_tools()}

    on, off = run(t), run(t, author=False)
    assert {"build_rete", "check_ontology", "suggest_vocabulary", "causal_diagram"} <= on
    assert not ({"build_rete", "check_ontology", "suggest_vocabulary", "causal_diagram"} & off)
    # the skills are served either way
    async def names(c):
        return sorted(s.frontmatter.name for s in await c.list_skills())
    assert run(names, author=False) == NAMES


# --- SPARQL, Turtle, Python, the worked example --------------------------------

def test_there_are_sparql_blocks_to_check():
    assert len(blocks("sparql")) >= 25


def _expects_no_rows(query):
    return query.lstrip().startswith("# no rows")


def test_every_sparql_block_parses():
    from rete_server import qlint

    with serve():
        for path, query in blocks("sparql"):
            doc = qlint.validate_query(with_prefixes(query))
            assert doc["ok"], f"{path.relative_to(SKILLS)}:\n{query}\n{doc['errors']}"


def test_every_sparql_block_runs_on_a_real_graph():
    async def t(c):
        report = []
        for path, query in blocks("sparql"):
            if "SERVICE" in query:
                continue                     # needs a live endpoint; parsed above
            r = await c.call_tool("sparql_query", {"dataset": KEY, "query": with_prefixes(query)},
                                  raise_on_error=False)
            where = f"{path.relative_to(SKILLS)}:\n{query}"
            assert not r.is_error, f"{where}\n{r.content[0].text}"
            doc = json.loads(r.content[0].text)
            rows = len(doc.get("table") or doc.get("triples") or []) or (1 if doc.get("boolean") else 0)
            skill = path.relative_to(SKILLS).parts[0]
            if skill in ("query", "network-analysis") and not _expects_no_rows(query):
                assert rows, f"no rows: {where}"
            report.append((skill, rows))
        return report

    report = run(t)
    assert len(report) >= 25


def test_the_documented_zero_row_trap_is_a_warning_and_the_fix_returns_rows():
    async def t(c):
        out = {}
        for key, q in (("bad", 'SELECT ?paper WHERE { ?paper ex:author ?a FILTER(CONTAINS(?paper, "p1")) }'),
                       ("good", 'SELECT ?paper WHERE { ?paper ex:author ?a FILTER(CONTAINS(STR(?paper), "p1")) }')):
            out[key] = tool_json(await c.call_tool("sparql_query", {"dataset": KEY, "query": with_prefixes(q)}))
        return out

    out = run(t)
    assert out["bad"]["table"] == [] and out["bad"]["warnings"][0]["function"] == "CONTAINS"
    assert out["good"]["table"] and out["good"]["warnings"] == []


def test_the_engine_issues_are_still_issues():
    """The skills say these are broken in 0.7.8. When one starts passing, the
    workaround in the skill is out of date: this test says so."""
    async def t(c):
        q = with_prefixes("CONSTRUCT { ?s ex:author ?o } WHERE { ?s ex:author ?o } LIMIT 2")
        construct = tool_json(await c.call_tool("sparql_query", {"dataset": KEY, "query": q}))
        trimmed = tool_json(await c.call_tool("sparql_query", {"dataset": KEY, "query": q, "limit": 2}))
        agg = tool_json(await c.call_tool("sparql_query", {"dataset": KEY, "query": with_prefixes(
            "SELECT ?a (SUM(?pg) AS ?s) (MAX(?pg) AS ?m) WHERE { ?p ex:author ?a . OPTIONAL { ?p ex:pages ?pg } } "
            "GROUP BY ?a")}))
        return construct, trimmed, agg

    construct, trimmed, agg = run(t)
    assert len(construct["triples"]) == 11, "CONSTRUCT honours LIMIT now: update the query skill"
    assert len(trimmed["triples"]) == 2 and trimmed["truncated"] is True
    alan = next(r for r in agg["table"] if r["a"].endswith("/alan"))
    assert alan.get("s") in (None, "") and str(alan["m"]) == "36", "SUM after OPTIONAL is bound now: update the skills"


def test_every_turtle_block_parses():
    found = blocks("turtle")
    assert len(found) >= 4
    for path, text in found:
        rete.build(text, "ttl")


def test_every_python_block_is_valid_python():
    found = blocks("python")
    assert found
    for path, text in found:
        ast.parse(text)


def test_the_web_page_template_is_safe_and_complete():
    page = (SKILLS / "web-pages/references/page.html").read_text(encoding="utf-8")
    assert "innerHTML" not in page, "graph data is untrusted: textContent only"
    for needle in ("importScripts", "registerTool", "pageTools", "og:title", "application/ld+json", "readOnlyHint"):
        assert needle in page
    script = (SKILLS / "web-pages/references/build-static.mjs").read_text(encoding="utf-8")
    assert "\\u003c" in script, "JSON in a script tag must not be able to close it"


def _worked():
    text = (SKILLS / "build-rete/references/worked-example.md").read_text(encoding="utf-8")
    parts = re.findall(r"```(\w+)\n(.*?)```", text, re.S)
    turtle = [b for lang, b in parts if lang == "turtle"]
    args = json.loads(next(b for lang, b in parts if lang == "json"))
    return turtle[0], turtle[1], args


def test_the_worked_example_builds_and_verifies_as_the_skill_says():
    data, shapes, args = _worked()

    async def t(c):
        built = tool_json(await c.call_tool("build_rete", {"rdf": data, **args}))
        key = built["dataset"]
        card = tool_json(await c.call_tool("dataset_card", {"dataset": key}))
        schema = tool_json(await c.call_tool("dataset_schema", {"dataset": key}))
        examples = tool_json(await c.call_tool("example_queries", {"dataset": key}))
        counts = []
        for ex in examples:
            doc = tool_json(await c.call_tool("sparql_query", {"dataset": key, "query": ex["sparql"]}))
            assert doc["warnings"] == [], ex["title"]
            counts.append(len(doc["table"]))
        shacl = tool_json(await c.call_tool("validate_shacl", {"dataset": key, "shapes": shapes}))
        found = tool_json(await c.call_tool("find_entities", {"dataset": key, "text": "Grace"}))
        return built, card, schema, counts, shacl, found

    built, card, schema, counts, shacl, found = run(t)
    assert built["quads"] == 34 and card["triple_count"] == 34
    for field in ("title", "license", "version", "creators", "publisher", "doi", "cite_as", "keywords", "source"):
        assert card[field] == _worked()[2]["card"][field], field
    assert len(card["queries"]) == 4
    classes = dict(schema["schema"]["classes"])
    assert classes == {"https://schema.org/ScholarlyArticle": 3, "https://schema.org/Person": 4}
    relations = {tuple(r[:3]): r[3] for r in schema["schema"]["relations"]}
    assert relations[("https://schema.org/ScholarlyArticle", "https://schema.org/author", "https://schema.org/Person")] == 5
    assert relations[("https://schema.org/ScholarlyArticle", "https://schema.org/citation",
                      "https://schema.org/ScholarlyArticle")] == 3
    assert counts == [3, 5, 2, 3]
    assert shacl["report"]["conforms"] is True
    assert [f["subject"] for f in found] == ["https://example.org/library/grace-hopper"]


def test_every_shape_in_the_skills_runs():
    data, _, args = _worked()
    shapes = [t for _, t in blocks("turtle") if "sh:NodeShape" in t]
    assert len(shapes) >= 2

    async def t(c):
        built = tool_json(await c.call_tool("build_rete", {"rdf": data, **args}))
        out = []
        for s in shapes:
            r = await c.call_tool("validate_shacl", {"dataset": built["dataset"], "shapes": s}, raise_on_error=False)
            assert not r.is_error, f"{s}\n{r.content[0].text}"
            out.append(json.loads(r.content[0].text)["report"]["conforms"])
        return out

    assert all(run(t)), "the shapes in the skills must conform on the skills' own worked example"


def test_the_worked_example_on_the_cli_card_shape():
    """The CLI card file takes the same curated fields; ``example_queries`` are plain strings."""
    _, _, args = _worked()
    builder = rete.Builder().add("<https://example.org/a> <https://example.org/b> <https://example.org/c> .", format="nt")
    card = dict(args["card"])
    builder.card(**card, example_queries=[e["sparql"] for e in args["examples"]])
    g = rete.open(builder.run())
    assert g.card()["title"] == "Early computing papers"


def test_every_json_block_is_json():
    found = blocks("json")
    assert len(found) >= 3
    for path, text in found:
        json.loads(text)


def test_the_graph_view_calls_in_the_network_skill_work():
    text = (SKILLS / "network-analysis/SKILL.md").read_text(encoding="utf-8")
    call = json.loads(re.search(r"```json\n(.*?)```", text, re.S).group(1))
    assert call["options"]["layout"] in {"cose", "layered", "concentric", "breadthfirst", "circle", "grid"}

    async def t(c):
        args = {**call, "dataset": KEY, "query": with_prefixes(call["query"])}
        opened = await c.call_tool("graph_view", args)
        view = opened.structured_content["view_id"]
        updated = await c.call_tool("graph_view_update", {
            "view_id": view, "highlight": ["https://example.org/p3"], "focus": "https://example.org/p3",
            "options": {"group_by": "namespace", "group_boxes": True}})
        schema = await c.call_tool("graph_view", {"dataset": KEY, "schema": True})
        state = await c.call_tool("graph_view_state", {"view_id": view})
        return opened.structured_content, updated, schema.structured_content, state

    opened, updated, schema, state = run(t)
    assert opened["nodes"] >= 3 and opened["edges"] >= 2, opened
    assert schema["nodes"] > 3
