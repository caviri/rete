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


def ms(value: float, digits: int = 2) -> str:
    if value >= 100:
        return f"{value:.0f} ms"
    if value >= 10:
        return f"{value:.1f} ms"
    return f"{value:.{digits}f} ms"


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
            "| Engine | Step | Time |",
            "|---|---|--:|",
            f"| **rete** | `Rete::open` - indexes already built in the file | **{ms(load['rete_ms'], 1)}** |",
            f"| Oxigraph | bulk-load N-Triples + build in-memory indexes | {ms(load['oxigraph_ms'], 0)} |",
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
            f"Median of {query_reps} warm runs.",
            "",
            "| Operator / form | rete | Oxigraph | rete vs oxi | rows | ok |",
            "|---|--:|--:|--:|--:|:--:|",
        ]
    )

    for row in queries:
        if "rete_error" in row or "oxigraph_error" in row:
            rete = row.get("rete_error", "-")
            oxi = row.get("oxigraph_error", "-")
            lines.append(f"| {row['name']} | {rete} | {oxi} | - | - | - |")
            continue
        rete_wins = row["rete_ms"] < row["oxigraph_ms"]
        rete_cell = maybe_bold(ms(row["rete_ms"]), rete_wins)
        oxi_cell = maybe_bold(ms(row["oxigraph_ms"]), not rete_wins)
        rows_cell = str(row["rete_rows"])
        if row["rete_rows"] != row["oxigraph_rows"]:
            rows_cell = f"{row['rete_rows']} / {row['oxigraph_rows']}"
        lines.append(
            "| {name} | {rete} | {oxi} | {ratio} | {rows} | {ok} |".format(
                name=row["name"].replace("|", "\\|"),
                rete=rete_cell,
                oxi=oxi_cell,
                ratio=speedup(row["speedup"]),
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
            "- rete now wins most aggregate, GROUP BY, DISTINCT, path-closure, and",
            "  sorted-pagination (top-k) shapes: the engine evaluates as a lazy",
            "  pipeline over integer slot rows and resolves terms only at projection.",
            "- Oxigraph still dominates small-LIMIT shapes over multi-pattern joins",
            "  (OPTIONAL, 3-way join, path sequences, expression scans): rete scans",
            "  each pattern once per query, where Oxigraph's planner probes indexes",
            "  per row. Closing that gap needs an index-nested-loop strategy under",
            "  small LIMITs, not more laziness.",
            "- Both engines are in the sub-ms-to-tens-of-ms range on this dataset;",
            "  the remaining gap is join *strategy* on a handful of shapes, not",
            "  evaluation overhead.",
            "",
            f"### Batch transitive reachability - `coauthor+` from {reach['seed_count']} seeds",
            "",
            "\"From each seed author, who is reachable through co-authorship?\" rete",
            "exposes this as a dedicated primitive (`rete reach`, `batch_reach_*`); on",
            "Oxigraph it is a `coauthor+` property path evaluated per seed.",
            "",
            "| Engine / mode | Time | vs rete-serial |",
            "|---|--:|--:|",
            f"| rete - `batch_reach_serial` (1 core) | {ms(reach['rete_serial_ms'], 1)} | 1.0x |",
            f"| **rete - `batch_reach_parallel` ({env.get('logical_cores', '?')} cores)** | **{ms(reach['rete_parallel_ms'], 1)}** | **{speedup(reach['parallel_speedup_vs_serial'])}** |",
            f"| Oxigraph - `coauthor+` property path, per seed | {ms(reach['oxigraph_ms'], 0)} | {speedup(reach['oxigraph_vs_rete_serial'])} |",
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


def replace_section(markdown: str, section: str) -> str:
    if START in markdown and END in markdown:
        before, rest = markdown.split(START, 1)
        _, after = rest.split(END, 1)
        return before.rstrip() + "\n\n" + section.rstrip() + "\n\n" + after.lstrip("\n")

    heading = "\n## Comparison vs Oxigraph"
    next_heading = "\n## Parallelism"
    start = markdown.find(heading)
    end = markdown.find(next_heading)
    if start == -1 or end == -1 or end <= start:
        raise SystemExit("could not find Oxigraph comparison section in benchmark doc")
    return markdown[: start + 1] + section + markdown[end:]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("json_file", type=Path)
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
    args.output.write_text(replace_section(source, section), encoding="utf-8")


if __name__ == "__main__":
    main()
