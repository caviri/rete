#!/usr/bin/env python3
"""Render the Oxigraph comparison section of docs/BENCHMARK.md from JSON.

Usage:
  uv run python scripts/render_benchmark_doc.py docs/benchmark-opencitations.json \
    --input docs/BENCHMARK.md --output docs/BENCHMARK.md

The JSON shape is the output of `rete-bench --json`, optionally enriched with a
`metadata` object for prose-only details such as the run date and dataset note.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


START = "<!-- benchmark:opencitations:start -->"
END = "<!-- benchmark:opencitations:end -->"
LUBM_START = "<!-- benchmark:lubm:start -->"
LUBM_END = "<!-- benchmark:lubm:end -->"


def ms(value: float, digits: int = 2) -> str:
    if value >= 100:
        return f"{value:.0f} ms"
    if value >= 10:
        return f"{value:.1f} ms"
    return f"{value:.{digits}f} ms"


def ms_pm(value: float, sd: float | None) -> str:
    """`2.41 ±0.05 ms`-style cell (falls back to plain when no spread known)."""
    if sd is None:
        return ms(value)
    if value >= 100:
        return f"{value:.0f} ±{sd:.0f} ms"
    if value >= 10:
        return f"{value:.1f} ±{sd:.1f} ms"
    return f"{value:.2f} ±{sd:.2f} ms"


def mib(num_bytes: float | None) -> str:
    if num_bytes is None:
        return "-"
    return f"{num_bytes / (1024 * 1024):.2f}"


def maybe_bold(value: str, bold: bool) -> str:
    return f"**{value}**" if bold else value


def speedup(value: float) -> str:
    return f"{value:.1f}x"


def render(report: dict) -> str:
    meta = report.get("metadata", {})
    inputs = report.get("inputs", {})
    env = report.get("environment", {})
    load = report["load_open"]
    queries = report["queries"]
    agreement = report["query_agreement"]
    reach = report["reachability"]

    title = meta.get("title", "Comparison vs Oxigraph (real OpenCitations network)")
    latest_run = meta.get("latest_run", "unknown")
    oxigraph_version = meta.get("oxigraph_version", "0.5")
    machine = meta.get("machine", f"{env.get('logical_cores', 'unknown')} logical cores")
    query_reps = env.get("query_repetitions", 5)
    reach_reps = env.get("reach_repetitions", 3)
    dataset = meta.get(
        "dataset",
        "the benchmark N-Triples input loaded into both engines",
    )
    dataset_note = meta.get("dataset_note", "")

    lines: list[str] = [
        START,
        f"## {title}",
        "",
        f"Dataset: {dataset}",
    ]
    if dataset_note:
        lines.extend(["", dataset_note])
    lines.extend(
        [
            "",
            "This pits rete (a queryable *file*) against Oxigraph (a full in-memory",
            "triplestore with a mature SPARQL planner). Honest summary: rete opens",
            "much faster because its indexes are prebuilt in the file, wins several",
            "scan/aggregate shapes, still loses where Oxigraph can stream, stop early,",
            "or apply a mature planner, and wins decisively on multi-source",
            "reachability.",
            "",
            "This section is generated from",
            f"`{meta.get('source_json', 'docs/benchmark-opencitations.json')}` with",
            "`scripts/render_benchmark_doc.py`. Latest run:",
            f"**{latest_run}**. **Oxigraph {oxigraph_version}**, in-memory store",
            f"(no RocksDB). Machine: {machine}.",
            "",
            "### Load / open (one-time)",
            "",
            "| Engine | Step | Time | Resident heap after load |",
            "|---|---|--:|--:|",
            f"| **rete** | `Rete::open` - indexes already built in the file | **{ms(load['rete_ms'], 1)}** | {mib(load.get('rete_heap_bytes'))} MiB |",
            f"| Oxigraph | bulk-load N-Triples + build in-memory indexes | {ms(load['oxigraph_ms'], 0)} | {mib(load.get('oxigraph_heap_bytes'))} MiB |",
        ]
    )
    rss_kb = load.get("process_peak_rss_kb")
    if rss_kb:
        lines.extend(
            [
                "",
                f"Process peak RSS after both loads (`VmHWM`): {rss_kb / 1024:.0f} MiB.",
            ]
        )
    lines.extend(
        [
            "",
            "rete's \"load\" just maps a file whose dictionary + permutation indexes",
            "already exist on disk; Oxigraph parses the triples and builds its indexes",
            "on every startup. This is the format's core promise: **publish once, open",
            "instantly, query in place**.",
            "",
            "### SPARQL operator coverage (both single-threaded)",
            "",
            f"{len(queries)} queries spanning supported forms and operators, run on both",
            "engines. Row counts are a cross-engine correctness check across the",
            "language surface, not just a speed race.",
            f"Median ±sd of {query_reps} warm runs; `peak heap` is each query's exact",
            "allocation high-water mark (counting allocator), engine-comparable.",
            "",
            "| Operator / form | rete | Oxigraph | rete vs oxi | peak heap MiB (rete / oxi) | rows | ok |",
            "|---|--:|--:|--:|--:|--:|:--:|",
        ]
    )

    for row in queries:
        if "rete_error" in row or "oxigraph_error" in row:
            rete = row.get("rete_error", "-")
            oxi = row.get("oxigraph_error", "-")
            lines.append(f"| {row['name']} | {rete} | {oxi} | - | - | - | - |")
            continue
        rete_wins = row["rete_ms"] < row["oxigraph_ms"]
        rete_cell = maybe_bold(ms_pm(row["rete_ms"], row.get("rete_ms_sd")), rete_wins)
        oxi_cell = maybe_bold(
            ms_pm(row["oxigraph_ms"], row.get("oxigraph_ms_sd")), not rete_wins
        )
        heap_cell = "{} / {}".format(
            mib(row.get("rete_peak_heap_bytes")),
            mib(row.get("oxigraph_peak_heap_bytes")),
        )
        rows_cell = str(row["rete_rows"])
        if row["rete_rows"] != row["oxigraph_rows"]:
            rows_cell = f"{row['rete_rows']} / {row['oxigraph_rows']}"
        lines.append(
            "| {name} | {rete} | {oxi} | {ratio} | {heap} | {rows} | {ok} |".format(
                name=row["name"].replace("|", "\\|"),
                rete=rete_cell,
                oxi=oxi_cell,
                ratio=speedup(row["speedup"]),
                heap=heap_cell,
                rows=rows_cell,
                ok="yes" if row.get("agree") else "no",
            )
        )

    lines.extend(
        [
            "",
            f"**{agreement['agree']} / {agreement['total']} identical row counts** across",
            "SELECT/ASK/CONSTRUCT/DESCRIBE, algebra operators, filters/functions,",
            "property paths, and aggregates.",
            "",
            "Reading the times honestly:",
            "",
            "- rete now wins or ties the large majority of shapes: aggregates,",
            "  GROUP BY, DISTINCT, path closures, sorted pagination (top-k), and",
            "  large UNION/VALUES/MINUS results. The engine evaluates as a lazy",
            "  pipeline over integer slot rows, switches to index-nested-loop",
            "  probes under small LIMIT/ASK demand, and resolves terms only at",
            "  projection.",
            "- Oxigraph keeps an edge on REGEX scans whose pattern needs the real",
            "  regex engine (rete's regex-lite has no literal prefilter; literal",
            "  patterns use a substring fast path) and, by fractions of a",
            "  millisecond, on ASK and the tightest LIMIT joins — floor effects,",
            "  not the orders-of-magnitude gaps from before the engine rework.",
            "",
            f"### Batch transitive reachability - `coauthor+` from {reach['seed_count']} seeds",
            "",
            "\"From each seed author, who is reachable through co-authorship?\" rete",
            "exposes this as a dedicated primitive (`rete reach`, `batch_reach_*`); on",
            "Oxigraph it is a `coauthor+` property path evaluated per seed.",
            "",
            "| Engine / mode | Time | vs rete-serial |",
            "|---|--:|--:|",
            f"| rete - `batch_reach_serial` (1 core) | {ms_pm(reach['rete_serial_ms'], reach.get('rete_serial_ms_sd'))} | 1.0x |",
            f"| **rete - `batch_reach_parallel` ({env.get('logical_cores', '?')} cores)** | **{ms_pm(reach['rete_parallel_ms'], reach.get('rete_parallel_ms_sd'))}** | **{speedup(reach['parallel_speedup_vs_serial'])}** |",
            f"| Oxigraph - `coauthor+` property path, per seed | {ms_pm(reach['oxigraph_ms'], reach.get('oxigraph_ms_sd'))} | {speedup(reach['oxigraph_vs_rete_serial'])} |",
            "",
            f"rete serial and parallel both reached {reach['rete_serial_total']:,} nodes;",
            f"Oxigraph touched {reach['oxigraph_total']:,} result cells. The dedicated",
            "parallel primitive is a different abstraction level from a general SPARQL",
            "property path, so read this as: use the right tool for multi-source reach.",
            "",
            "### Reproduce",
            "",
            "```sh",
            "# In the dev container (Docker). The OpenCitations + synthetic-enrichment data",
            "# comes from scripts/fetch_opencitations.py + scripts/enrich.py (-> enriched-all.nt).",
            "# Sanitize malformed compound-DOI IRIs so both engines load identical data:",
            "grep -vE \"<[^>]* [^>]*>\" data/opencitations/enriched-all.nt \\",
            "  > data/opencitations/enriched-clean.nt",
            "./target/release/rete build data/opencitations/enriched-clean.nt \\",
            "  -o data/opencitations/enriched-clean.rete",
            "",
            "cargo build --release -p rete-bench",
            "./target/release/rete-bench --json data/opencitations/enriched-clean.rete \\",
            "  data/opencitations/enriched-clean.nt 300 > docs/benchmark-opencitations.json",
            "uv run python scripts/render_benchmark_doc.py docs/benchmark-opencitations.json \\",
            "  --input docs/BENCHMARK.md --output docs/BENCHMARK.md",
            "cargo run -p docgen",
            "```",
            "",
            "The `rete-bench` crate pulls in Oxigraph only for this comparison; its",
            "in-memory store needs no RocksDB/clang, so `default-features = false` keeps",
            "the build light.",
            END,
        ]
    )
    return "\n".join(lines) + "\n"


def render_lubm(report: dict, source_json: str) -> str:
    load = report["load_open"]
    queries = report["queries"]
    agreement = report["query_agreement"]
    lines: list[str] = [
        LUBM_START,
        "## LUBM-style benchmark (standard 14 queries)",
        "",
        f"The [LUBM](http://swat.cse.lehigh.edu/projects/lubm/) data model and its 14",
        "standard queries, on identical pre-materialized data in both engines.",
        f"LUBM({report['universities']}): {report['base_triples']:,} generated +",
        f"{report['materialized_triples']:,} RDFS/OWL-RL-materialized =",
        f"{report['total_triples']:,} triples (`.rete`: {report['rete_bytes']:,} bytes).",
        "",
        "**Read the caveats:** the generator is a faithful *reimplementation* of",
        "UBA's documented cardinalities, not the official Java tool, so counts are",
        "not comparable to published LUBM figures; inference is rete's RDFS/OWL-RL",
        "subset, applied up front to the data both engines load — so",
        "restriction-defined classes (Q12's `Chair`) are empty on both sides by",
        "construction. The correctness anchor is **cross-engine row parity**:",
        f"**{agreement['agree']} / {agreement['total']}** queries agree.",
        "",
        "| Engine | Load | Resident heap |",
        "|---|--:|--:|",
        f"| **rete** `Rete::open` | **{ms(load['rete_ms'], 1)}** | {mib(load.get('rete_heap_bytes'))} MiB |",
        f"| Oxigraph bulk-load | {ms(load['oxigraph_ms'], 0)} | {mib(load.get('oxigraph_heap_bytes'))} MiB |",
        "",
        "| Query | rete | Oxigraph | rete vs oxi | peak heap MiB (rete / oxi) | rows | ok |",
        "|---|--:|--:|--:|--:|--:|:--:|",
    ]
    for row in queries:
        rete_wins = row["rete_ms"] < row["oxigraph_ms"]
        lines.append(
            "| {name} | {rete} | {oxi} | {ratio} | {heap} | {rows} | {ok} |".format(
                name=row["name"].replace("|", "\\|"),
                rete=maybe_bold(ms_pm(row["rete_ms"], row.get("rete_ms_sd")), rete_wins),
                oxi=maybe_bold(
                    ms_pm(row["oxigraph_ms"], row.get("oxigraph_ms_sd")), not rete_wins
                ),
                ratio=speedup(row["speedup"]),
                heap="{} / {}".format(
                    mib(row.get("rete_peak_heap_bytes")),
                    mib(row.get("oxigraph_peak_heap_bytes")),
                ),
                rows=row["rows"],
                ok="yes" if row.get("agree") else "no",
            )
        )
    lines.extend(
        [
            "",
            "Reproduce: `cargo run --release -p rete-bench -- --json --lubm 1 >",
            f"{source_json}` then re-render this doc.",
            LUBM_END,
        ]
    )
    return "\n".join(lines) + "\n"


def replace_between(markdown: str, section: str, start: str, end: str) -> str | None:
    if start in markdown and end in markdown:
        before, rest = markdown.split(start, 1)
        _, after = rest.split(end, 1)
        return before.rstrip() + "\n\n" + section.rstrip() + "\n\n" + after.lstrip("\n")
    return None


def replace_section(markdown: str, section: str) -> str:
    replaced = replace_between(markdown, section, START, END)
    if replaced is not None:
        return replaced

    heading = "\n## Comparison vs Oxigraph"
    next_heading = "\n## Parallelism"
    start = markdown.find(heading)
    end = markdown.find(next_heading)
    if start == -1 or end == -1 or end <= start:
        raise SystemExit("could not find Oxigraph comparison section in benchmark doc")
    return markdown[: start + 1] + section + markdown[end:]


def insert_lubm(markdown: str, section: str) -> str:
    replaced = replace_between(markdown, section, LUBM_START, LUBM_END)
    if replaced is not None:
        return replaced
    # First insertion: right after the Oxigraph comparison section.
    anchor = markdown.find(END)
    if anchor == -1:
        raise SystemExit("render the Oxigraph section before adding the LUBM section")
    anchor += len(END)
    return markdown[:anchor] + "\n\n" + section.rstrip() + markdown[anchor:]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("json_file", type=Path)
    parser.add_argument(
        "--lubm",
        type=Path,
        default=None,
        help="optional rete-bench --lubm --json report to render as its own section",
    )
    parser.add_argument("--input", type=Path, default=Path("docs/BENCHMARK.md"))
    parser.add_argument("--output", type=Path, default=Path("docs/BENCHMARK.md"))
    parser.add_argument("--section-only", action="store_true")
    args = parser.parse_args()

    report = json.loads(args.json_file.read_text(encoding="utf-8"))
    report.setdefault("metadata", {})
    report["metadata"].setdefault("source_json", args.json_file.as_posix())
    section = render(report)
    if args.section_only:
        print(section, end="")
        return

    source = args.input.read_text(encoding="utf-8")
    out = replace_section(source, section)
    if args.lubm is not None:
        lubm_report = json.loads(args.lubm.read_text(encoding="utf-8"))
        out = insert_lubm(out, render_lubm(lubm_report, args.lubm.as_posix()))
    args.output.write_text(out, encoding="utf-8")


if __name__ == "__main__":
    main()
