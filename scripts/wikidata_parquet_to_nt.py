#!/usr/bin/env python3
"""Convert the real Wikidata "truthy" dump (as Parquet on Hugging Face) into
N-Triples for `rete build` — a genuine, gigabyte-scale linked-data graph for
testing, no Wikidata Query Service rate limits.

Source: https://huggingface.co/datasets/piebro/wikidata-extraction — the full
`latest-truthy` dump converted to ~80 Parquet partitions (~900 MB each, ~70 GB
total) with columns `subject, predicate, object, language`. This reads it with
DuckDB (httpfs streams the remote files; no full download needed for a bounded
slice) and writes standard N-Triples.

Scale is set by `--parts` (how many ~900 MB partitions to draw from) and/or
`--limit` (a hard cap on emitted triples). A ~1 GB graph is a `--limit` of
roughly 12 million triples (≈1.25 GB N-Triples, builds to a ~110 MB `.rete`).
The slice is a real cross-section of all of Wikidata (people, places, works,
taxa, …), not a curated subset — for a biology-specific graph use
`scripts/fetch_wikidata_bio.py` instead.

Literal datatypes (dropped by the source Parquet) are recovered. `--datatypes`:
* `auto` (default): the authoritative property→datatype map — a local cache
  (`--datatype-cache`) if present, else one WDQS `wikibase:propertyType` query
  (with 429 backoff), cached for next time — giving dates `xsd:dateTime`,
  quantities `xsd:decimal`, coordinates `geo:wktLiteral`; if that is
  unavailable it falls back to the heuristic below;
* `heuristic`: offline value inference — only the unambiguous `xsd:dateTime`
  (ISO timestamps) and `geo:wktLiteral` (WKT geometries); numbers stay plain,
  since a bare number can't be told from a numeric external-id without the map;
* `none`: all literals plain.
Monolingual text keeps its language tag and entity values are IRIs regardless.

Requires DuckDB:  pip install --break-system-packages duckdb

Usage:
  uv run python scripts/wikidata_parquet_to_nt.py --limit 12000000 -o data/wd.nt   # ~1 GB
  uv run python scripts/wikidata_parquet_to_nt.py --parts 1 -o data/wd.nt          # one whole partition
  uv run python scripts/wikidata_parquet_to_nt.py --local-dir /data/triplets       # already-downloaded parquet
Then: rete build data/wd.nt -o wd.rete
"""

from __future__ import annotations

import argparse
import os
import sys
import time
import urllib.parse
import urllib.request

HF_BASE = (
    "https://huggingface.co/datasets/piebro/wikidata-extraction/resolve/main/triplets"
)

GEO_WKT = "http://www.opengis.net/ont/geosparql#wktLiteral"
XSD = "http://www.w3.org/2001/XMLSchema#"

# The source Parquet dropped literal datatypes. They are recoverable
# authoritatively: every Wikidata property has a fixed Wikibase datatype, and
# the truthy `wdt:` literal for each maps to a known xsd/geo type. Only these
# three become *typed* literals; String/ExternalId/CommonsMedia/Math/… are
# plain literals, Monolingualtext is language-tagged (the `language` column),
# and Url/WikibaseItem/… are IRIs (the http:// check below handles them).
WIKIBASE_TO_DTYPE = {
    "Time": f"{XSD}dateTime",
    "Quantity": f"{XSD}decimal",
    "GlobeCoordinate": GEO_WKT,
}

WDQS = "https://query.wikidata.org/sparql"
USER_AGENT = "rete-demo/0.1 (https://github.com/caviri/rete; parquet datatype lookup)"


def _wdqs_property_types_csv() -> str | None:
    """The raw `P-id,WikibaseType` CSV for all properties from WDQS, with a
    backoff retry on 429 (the service rate-limits to ~1 req/min under load).
    `None` on failure."""
    q = "SELECT ?p ?t WHERE { ?p wikibase:propertyType ?t }"
    url = WDQS + "?" + urllib.parse.urlencode({"query": q})
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "text/csv"})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return r.read().decode("utf-8")
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt < 2:
                print("  WDQS rate-limited (429); waiting 65 s…", file=sys.stderr)
                time.sleep(65)
                continue
            print(f"  datatype lookup failed (HTTP {e.code})", file=sys.stderr)
            return None
        except Exception as e:  # noqa: BLE001 — best-effort
            print(f"  datatype lookup failed ({e})", file=sys.stderr)
            return None
    return None


def fetch_property_datatypes(cache: str | None) -> dict[str, str]:
    """Map bare property id (e.g. 'P569') -> recovered literal datatype IRI.

    Reads the authoritative `P-id,WikibaseType` CSV from a local `cache` file
    if present (so a build needs no network and survives WDQS outages); else
    fetches it from WDQS (`wikibase:propertyType`, ~13.5k properties, one
    query, with 429 backoff) and writes the cache for next time. `{}` on
    failure with no cache — the converter then emits plain literals."""
    text: str | None = None
    if cache and os.path.exists(cache):
        text = open(cache, encoding="utf-8").read()
        print(f"  using cached property datatypes: {cache}", file=sys.stderr)
    else:
        text = _wdqs_property_types_csv()
        if text and cache:
            os.makedirs(os.path.dirname(cache) or ".", exist_ok=True)
            with open(cache, "w", encoding="utf-8") as f:
                f.write(text)
            print(f"  cached property datatypes to {cache}", file=sys.stderr)
    if not text:
        print("  no datatype map; emitting plain literals", file=sys.stderr)
        return {}
    out: dict[str, str] = {}
    for line in text.splitlines()[1:]:  # skip CSV header
        p, _, t = line.partition(",")
        pid = p.rsplit("/", 1)[-1]
        wb = t.rsplit("#", 1)[-1]
        if pid and wb in WIKIBASE_TO_DTYPE:
            out[pid] = WIKIBASE_TO_DTYPE[wb]
    return out


import re

# Offline fallback when the authoritative property→datatype map is unavailable
# (WDQS/HF rate-limited). Only the *unambiguous* shapes are inferred from the
# value: an ISO timestamp and a WKT geometry can't be confused with anything
# else. Quantities are deliberately NOT inferred — a bare number is
# indistinguishable from a numeric external-id without the property map, so
# those stay plain literals rather than risk mis-typing.
_DATETIME_RE = re.compile(r"^[+-]?\d{1,4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z?$")
_WKT_RE = re.compile(r"^(?:Point|POLYGON|LINESTRING|MULTIPOINT|MULTIPOLYGON|MULTILINESTRING|GEOMETRYCOLLECTION)\(", re.IGNORECASE)


def infer_datatype(obj: str) -> str | None:
    """Heuristic datatype for an object value — only the unambiguous ones."""
    if _DATETIME_RE.match(obj):
        return f"{XSD}dateTime"
    if _WKT_RE.match(obj):
        return GEO_WKT
    return None


def nt_object(obj: str, lang: str | None, dtype: str | None) -> str:
    """Format a parquet `object` as an N-Triples object term: an IRI in angle
    brackets, a blank node as-is, a language-tagged literal, a `dtype`-typed
    literal (recovered from the predicate), else a plain literal."""
    if obj.startswith(("http://", "https://")) and not lang:
        return f"<{obj}>"
    if obj.startswith("_:"):
        return obj
    esc = (
        obj.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
    )
    if lang:
        return f'"{esc}"@{lang}'
    if dtype:
        return f'"{esc}"^^<{dtype}>'
    return f'"{esc}"'


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--parts", type=int, default=1, help="number of ~900 MB partitions to read (1..80)")
    ap.add_argument("--part-index", type=int, default=None,
                    help="process ONLY this single partition (0..79) — for sharded XXL builds, one shard per partition; overrides --parts")
    ap.add_argument("--limit", type=int, default=None, help="hard cap on emitted triples (~12M ≈ 1 GB)")
    ap.add_argument("--local-dir", default=None, help="read part_*.parquet from a local dir instead of HF")
    ap.add_argument(
        "--datatypes",
        choices=["auto", "heuristic", "none"],
        default="auto",
        help="auto: authoritative property map (cache→WDQS) then value heuristics; "
        "heuristic: value heuristics only (offline, dates+coordinates); none: plain literals",
    )
    ap.add_argument(
        "--datatype-cache",
        default="data/wd_property_types.csv",
        help="local cache of the property→datatype CSV (fetched once, reused)",
    )
    ap.add_argument("-o", "--output", default="data/wikidata.nt")
    args = ap.parse_args()

    try:
        import duckdb
    except ModuleNotFoundError:
        sys.exit("duckdb not installed — run: pip install --break-system-packages duckdb")

    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)

    # Recover literal datatypes (time/quantity/coordinate) from the property
    # map, unless disabled. Keyed by bare predicate id ('P569' → xsd:dateTime).
    dtypes: dict[str, str] = {}
    use_heuristic = args.datatypes == "heuristic"
    if args.datatypes == "auto":
        print("resolving property datatypes…", file=sys.stderr)
        dtypes = fetch_property_datatypes(args.datatype_cache)
        if dtypes:
            print(f"  {len(dtypes)} properties carry a typed literal (authoritative)", file=sys.stderr)
        else:
            use_heuristic = True
            print("  no property map — falling back to value heuristics", file=sys.stderr)
    if use_heuristic:
        print("  inferring dateTime/wktLiteral from values (heuristic)", file=sys.stderr)

    # one specific partition (sharded build) or the first `--parts` of them
    idxs = [args.part_index] if args.part_index is not None else list(range(args.parts))
    con = duckdb.connect()
    if args.local_dir:
        sources = [
            os.path.join(args.local_dir, f"part_{i:04d}.parquet") for i in idxs
        ]
        sources = [s for s in sources if os.path.exists(s)]
        if not sources:
            sys.exit(f"no part_*.parquet found in {args.local_dir}")
    else:
        con.execute("INSTALL httpfs; LOAD httpfs;")
        sources = [f"{HF_BASE}/part_{i:04d}.parquet" for i in idxs]

    src_list = ", ".join(f"'{s}'" for s in sources)
    limit_sql = f" LIMIT {args.limit}" if args.limit else ""
    print(
        f"reading {len(sources)} partition(s)"
        + (f", capped at {args.limit:,} triples" if args.limit else "")
        + f"\nwriting {args.output}…",
        file=sys.stderr,
    )

    cur = con.execute(
        f"SELECT subject, predicate, object, language "
        f"FROM read_parquet([{src_list}]){limit_sql}"
    )

    written = 0
    with open(args.output, "w", encoding="utf-8", newline="\n") as f:
        while True:
            batch = cur.fetchmany(100_000)
            if not batch:
                break
            lines = []
            for subject, predicate, obj, lang in batch:
                if subject is None or predicate is None or obj is None:
                    continue
                if dtypes:
                    dtype = dtypes.get(predicate.rsplit("/", 1)[-1])
                elif use_heuristic and not lang and not obj.startswith(("http://", "https://", "_:")):
                    dtype = infer_datatype(obj)
                else:
                    dtype = None
                lines.append(f"<{subject}> <{predicate}> {nt_object(obj, lang, dtype)} .\n")
            f.write("".join(lines))
            written += len(lines)
            if written % 1_000_000 < 100_000:
                print(f"  {written:,} triples…", file=sys.stderr)

    mb = os.path.getsize(args.output) / (1024 * 1024)
    print(
        f"wrote {written:,} triples ({mb:.0f} MB) to {args.output}\n"
        f"next: rete build {args.output} -o "
        f"{os.path.splitext(args.output)[0]}.rete",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
