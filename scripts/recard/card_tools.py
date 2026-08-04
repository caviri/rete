#!/usr/bin/env python3
"""Card surgery for the re-card pipeline: carry curated fields, classify, verify.

`rete repyramid --card` (and `rete build --card`) re-derive a Dataset Card from
the data, but they take the **curated** half only from flags or `--card-file` —
a bare `--card` silently drops the publisher's `title`/`source`/`license`. So a
re-card must read the old card first and hand the curated fields back in. That
is what `curated` does; `classify` decides which files need the work at all, and
`verify` is the gate that says the new card is actually better than the old one.

Subcommands (all read/write JSON; `-` means stdin/stdout):

  curated  IN [-o OUT]      old `rete card --json` -> a `--card-file` document
  classify IN               one card -> one verdict row (JSON, one line)
  report   IN               a stream of verdict rows -> TSV table + summary
  verify   --old A --new B  gate: curated fields carried + starter queries answer

`(no dataset card)` — what the CLI prints for a cardless file — is accepted
wherever a card is read, and classified as `CARDLESS`.
"""

from __future__ import annotations

import argparse
import json
import sys

# Exactly the top level of `CardInput` (crates/rete-cli/src/commands/card.rs).
# It is `deny_unknown_fields`, so a stray key here is a hard build error — the
# list must stay in step with the struct, and the `verify` gate re-reads these
# same keys out of the rebuilt card to prove none was dropped.
CURATED_FIELDS = [
    "title",
    "description",
    "license",
    "source",
    "created",
    "version",
    "creators",
    "publisher",
    "canonical_url",
    "sparql_endpoint",
    "source_date",
    "derived_from",
    "doi",
    "cite_as",
    "keywords",
    "theme",
    "extra",
    "example_queries",
]

# Starter queries that may legitimately return zero rows on a healthy file:
# `top-dangling` asks for IRIs referenced but never described, and a
# fully-described graph has none. Everything else returning zero means the card
# is describing a graph the query cannot see — the bug this pipeline exists for.
LEGITIMATELY_EMPTY = {"top-dangling"}

# Verdicts, worst first. `todo` is the set the batch driver runs.
STATUS_ORDER = ["CARDLESS", "ZERO-ROWS", "MIXED-HIDDEN", "DATED", "CURRENT", "UNREADABLE"]
TODO_STATUSES = {"CARDLESS", "ZERO-ROWS", "MIXED-HIDDEN", "DATED"}


def read_card(path: str) -> dict | None:
    """Parse a card document. Returns None for a cardless file.

    Accepts the raw `rete card --json` bytes, or the `(no dataset card)` line
    the CLI prints instead when the metadata section is empty.
    """
    raw = sys.stdin.read() if path == "-" else open(path, encoding="utf-8").read()
    text = raw.strip()
    if not text or text.startswith("(no dataset card)"):
        return None
    # `card-url` writes its byte-accounting line to stderr, but be forgiving if a
    # caller merged the streams: take the JSON object only.
    start = text.find("{")
    if start < 0:
        return None
    return json.loads(text[start:])


def write_json(obj, path: str) -> None:
    text = json.dumps(obj, ensure_ascii=False, indent=2, sort_keys=True)
    if path == "-":
        sys.stdout.write(text + "\n")
    else:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")


def curated_of(card: dict) -> dict:
    """The curated half of a card, shaped as a `--card-file` document.

    Empty values are dropped rather than written as `null`/`[]`: the card
    serializer omits them too, so a round trip through this function is a no-op
    on the file's bytes.
    """
    out = {}
    for key in CURATED_FIELDS:
        value = card.get(key)
        if value in (None, "", [], {}):
            continue
        out[key] = value
    return out


def _queries(card: dict) -> list[dict]:
    return card.get("queries") or []


def _query_rows(card: dict) -> list[dict]:
    build = card.get("build") or {}
    costs = build.get("query_costs") or {}
    return costs.get("queries") or []


def classify(card: dict | None, key: str, url: str, error: str = "") -> dict:
    """One dataset's verdict. Reads only what the card tier already fetched."""
    row = {"key": key, "url": url, "status": "", "reason": "", "triples": None,
           "quads": None, "named_graphs": None, "queries": 0, "graph_scoped": 0,
           "has_build_record": False, "title": None, "format_version": None}
    if error:
        row["status"] = "UNREADABLE"
        row["reason"] = error
        return row
    if card is None:
        row["status"] = "CARDLESS"
        row["reason"] = "no dataset card — nothing describes this file"
        return row

    queries = _queries(card)
    scoped = [q for q in queries if "GRAPH" in (q.get("sparql") or "")]
    triples = card.get("triple_count")
    named = card.get("named_graph_count") or 0
    row.update(
        triples=triples,
        quads=card.get("quad_count"),
        named_graphs=named,
        queries=len(queries),
        graph_scoped=len(scoped),
        has_build_record=bool(card.get("build")),
        title=card.get("title"),
        format_version=card.get("format_version"),
    )

    named_only = named > 0 and not triples
    if named_only and (not queries or len(scoped) < len(queries)):
        row["status"] = "ZERO-ROWS"
        row["reason"] = (
            "every statement is in a named graph and the default graph is empty, "
            f"but {len(queries) - len(scoped)} of {len(queries)} starter queries "
            "scan the default graph — they return zero rows"
            if queries
            else "named-graph-only file with no starter queries at all"
        )
        return row
    if triples and named and not scoped:
        row["status"] = "MIXED-HIDDEN"
        row["reason"] = (
            f"{named} named graph(s) alongside the default graph, but no starter "
            "query looks inside them — half the file is invisible"
        )
        return row

    stale = []
    if not card.get("build"):
        stale.append("no build record (provenance, params, measured query costs)")
    if not card.get("top_n"):
        stale.append("no top-N cap recorded (profile predates the field)")
    if not queries:
        stale.append("no starter queries")
    elif not any(q.get("id") == "ov-one-row" for q in queries):
        stale.append("no ov-one-row smoke query")
    if stale:
        row["status"] = "DATED"
        row["reason"] = ", ".join(stale)
        return row

    row["status"] = "CURRENT"
    row["reason"] = (
        f"{len(scoped)}/{len(queries)} graph-scoped starter queries + build record"
        if named
        else f"{len(queries)} default-graph starter queries + build record"
    )
    return row


def cmd_curated(args) -> int:
    card = read_card(args.input)
    if card is None:
        # A cardless file has nothing to carry; an empty document is a valid
        # `--card-file` and keeps the caller's command line uniform.
        write_json({}, args.output)
        print("card_tools: no existing card — writing an empty curated document",
              file=sys.stderr)
        return 0
    curated = curated_of(card)
    write_json(curated, args.output)
    print(f"card_tools: carrying {len(curated)} curated field(s): "
          f"{', '.join(sorted(curated)) or '(none)'}", file=sys.stderr)
    return 0


def cmd_classify(args) -> int:
    try:
        card = read_card(args.input)
        error = ""
    except (OSError, ValueError) as exc:
        card, error = None, str(exc)
    row = classify(card, args.key, args.url, error)
    sys.stdout.write(json.dumps(row, ensure_ascii=False) + "\n")
    return 0


def cmd_report(args) -> int:
    rows = []
    stream = sys.stdin if args.input == "-" else open(args.input, encoding="utf-8")
    for line in stream:
        line = line.strip()
        if line:
            rows.append(json.loads(line))
    rank = {s: i for i, s in enumerate(STATUS_ORDER)}
    rows.sort(key=lambda r: (rank.get(r["status"], 99), r["key"]))

    header = ["status", "key", "triples", "quads", "named_graphs", "queries",
              "graph_scoped", "build", "reason"]
    print("\t".join(header))
    for r in rows:
        print("\t".join(str(x) for x in [
            r["status"], r["key"], r["triples"], r["quads"], r["named_graphs"],
            r["queries"], r["graph_scoped"], "yes" if r["has_build_record"] else "no",
            r["reason"],
        ]))

    counts = {}
    for r in rows:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    print("", file=sys.stderr)
    print(f"# {len(rows)} dataset(s)", file=sys.stderr)
    for status in STATUS_ORDER:
        if status in counts:
            print(f"#   {status:<13} {counts[status]}", file=sys.stderr)

    if args.todo:
        todo = [r["key"] for r in rows if r["status"] in TODO_STATUSES]
        with open(args.todo, "w", encoding="utf-8") as fh:
            fh.write("".join(f"{k}\n" for k in todo))
        print(f"# wrote {len(todo)} key(s) needing work -> {args.todo}", file=sys.stderr)
    if args.json:
        write_json(rows, args.json)
    return 0


def cmd_verify(args) -> int:
    """The gate. Fails loudly; a warning here would defeat the point."""
    old = read_card(args.old) if args.old else None
    new = read_card(args.new)
    problems: list[str] = []

    if new is None:
        print("verify: the rebuilt file has NO card", file=sys.stderr)
        return 1

    # 1. Curated fields carried across. Losing a publisher's title while
    #    "upgrading" their card is the failure mode this whole tool exists to
    #    avoid, so it is an error, not a warning.
    if old is not None:
        was, now = curated_of(old), curated_of(new)
        for key, value in was.items():
            if key not in now:
                problems.append(f"curated field dropped: {key} = {value!r}")
            elif now[key] != value:
                problems.append(f"curated field changed: {key}: {value!r} -> {now[key]!r}")

    # 2. Every starter query answers. The measured row counts are already in the
    #    file (build-info `query_costs`, written by the same build that derived
    #    the queries), so this costs nothing to check and is the real proof that
    #    a named-graph-only file no longer ships guaranteed-zero-rows SPARQL.
    measured = _query_rows(new)
    queries = _queries(new)
    if not queries:
        problems.append("the rebuilt card carries no starter queries")
    elif not measured:
        problems.append(
            "no measured query costs in the build record — rebuild without "
            "--no-card-costs so the row counts are recorded"
        )
    else:
        allow = set(LEGITIMATELY_EMPTY) | set(args.allow_empty or [])
        seen = {q["id"]: q.get("rows", 0) for q in measured}
        for qid, rows in seen.items():
            if rows == 0 and qid not in allow:
                problems.append(f"starter query {qid} returns ZERO rows")
        if seen.get("ov-one-row") != 1:
            problems.append(
                "the ov-one-row smoke query did not return exactly one row "
                f"(got {seen.get('ov-one-row')!r}) — the file does not answer"
            )

    # 3. Scope sanity: on a named-graph-only file every query must look inside a
    #    graph. Belt and braces over (2) — a query can return rows by accident.
    if (new.get("named_graph_count") or 0) > 0 and not new.get("triple_count"):
        unscoped = [q["id"] for q in queries if "GRAPH" not in (q.get("sparql") or "")]
        if unscoped:
            problems.append(
                "named-graph-only file still ships default-graph queries: "
                + ", ".join(unscoped)
            )

    if problems:
        print("verify: FAILED", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    rows = {q["id"]: q.get("rows", 0) for q in measured}
    print(f"verify: ok — {len(queries)} starter queries, rows: "
          + ", ".join(f"{k}={v}" for k, v in rows.items()), file=sys.stderr)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("curated", help="old card -> a --card-file document")
    p.add_argument("input")
    p.add_argument("-o", "--output", default="-")
    p.set_defaults(func=cmd_curated)

    p = sub.add_parser("classify", help="one card -> one verdict row")
    p.add_argument("input")
    p.add_argument("--key", default="")
    p.add_argument("--url", default="")
    p.set_defaults(func=cmd_classify)

    p = sub.add_parser("report", help="verdict rows -> TSV table")
    p.add_argument("input", nargs="?", default="-")
    p.add_argument("--todo", help="also write the keys needing work, one per line")
    p.add_argument("--json", help="also write the rows as a JSON array")
    p.set_defaults(func=cmd_report)

    p = sub.add_parser("verify", help="gate the rebuilt card")
    p.add_argument("--old")
    p.add_argument("--new", required=True)
    p.add_argument("--allow-empty", nargs="*",
                   help="starter query ids allowed to return zero rows")
    p.set_defaults(func=cmd_verify)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
