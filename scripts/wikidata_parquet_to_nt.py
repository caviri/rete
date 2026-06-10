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

Literal datatypes (dropped by the source Parquet) are recovered authoritatively
from each property's Wikibase datatype (one WDQS query): dates become
`xsd:dateTime`, quantities `xsd:decimal`, coordinates `geo:wktLiteral`; strings
stay plain, monolingual text keeps its language tag, entity values are IRIs.
Pass `--no-datatypes` to skip the lookup and emit plain literals.

Requires DuckDB:  pip install --break-system-packages duckdb

Usage:
  uv run python scripts/wikidata_parquet_to_nt.py --limit 12000000 -o data/wd.nt   # ~1 GB
  uv run python scripts/wikidata_parquet_to_nt.py --parts 1 -o data/wd.nt          # one whole partition
  uv run python scripts/wikidata_parquet_to_nt.py --local-dir /data/triplets       # already-downloaded parquet
Then: rete build data/wd.nt -o wd.rete
"""

from __future__ import annotations

import argparse
import sys
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


def fetch_property_datatypes() -> dict[str, str]:
    """Map bare property id (e.g. 'P569') -> recovered literal datatype IRI,
    from WDQS `wikibase:propertyType` (all ~13.5k properties, one query). Only
    properties whose values are typed literals appear; on any network failure
    return {} (the converter then emits plain literals, as before)."""
    q = "SELECT ?p ?t WHERE { ?p wikibase:propertyType ?t }"
    url = WDQS + "?" + urllib.parse.urlencode({"query": q})
    req = urllib.request.Request(
        url, headers={"User-Agent": USER_AGENT, "Accept": "text/csv"}
    )
    out: dict[str, str] = {}
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            text = r.read().decode("utf-8")
    except Exception as e:  # noqa: BLE001 — best-effort; plain literals if it fails
        print(f"  datatype lookup failed ({e}); emitting plain literals", file=sys.stderr)
        return out
    for line in text.splitlines()[1:]:  # skip CSV header
        p, _, t = line.partition(",")
        pid = p.rsplit("/", 1)[-1]
        wb = t.rsplit("#", 1)[-1]
        if pid and wb in WIKIBASE_TO_DTYPE:
            out[pid] = WIKIBASE_TO_DTYPE[wb]
    return out


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
    ap.add_argument("--limit", type=int, default=None, help="hard cap on emitted triples (~12M ≈ 1 GB)")
    ap.add_argument("--local-dir", default=None, help="read part_*.parquet from a local dir instead of HF")
    ap.add_argument(
        "--no-datatypes",
        action="store_true",
        help="skip the WDQS property-datatype lookup; emit all literals as plain",
    )
    ap.add_argument("-o", "--output", default="data/wikidata.nt")
    args = ap.parse_args()

    try:
        import duckdb
    except ModuleNotFoundError:
        sys.exit("duckdb not installed — run: pip install --break-system-packages duckdb")

    import os

    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)

    # Recover literal datatypes (time/quantity/coordinate) from the property
    # map, unless disabled. Keyed by bare predicate id ('P569' → xsd:dateTime).
    dtypes: dict[str, str] = {}
    if not args.no_datatypes:
        print("fetching property datatypes from WDQS…", file=sys.stderr)
        dtypes = fetch_property_datatypes()
        print(f"  {len(dtypes)} properties carry a typed literal", file=sys.stderr)

    con = duckdb.connect()
    if args.local_dir:
        sources = [
            os.path.join(args.local_dir, f"part_{i:04d}.parquet") for i in range(args.parts)
        ]
        sources = [s for s in sources if os.path.exists(s)]
        if not sources:
            sys.exit(f"no part_*.parquet found in {args.local_dir}")
    else:
        con.execute("INSTALL httpfs; LOAD httpfs;")
        sources = [f"{HF_BASE}/part_{i:04d}.parquet" for i in range(args.parts)]

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
                dtype = dtypes.get(predicate.rsplit("/", 1)[-1]) if dtypes else None
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
