"""Stream the GoTriple metadata dump (per-discipline *.jsonl.gz) into Parquet.

The dataset (Zenodo 18185971, CC0) ships one gzipped JSON-Lines file per
discipline, one JSON document per SSH publication. This converts each file to a
zstd Parquet table (`parquet/<discipline>.parquet`, glob-scannable), streaming
line-by-line so peak memory stays bounded regardless of file size.

Schema: analytically useful scalars are flattened to typed columns (doi = first
non-empty DOI — the cross-dataset join key; title = first headline text;
discipline from the filename; primary_topic = highest-confidence topic), and
every nested GoTriple field is kept whole as a JSON-string column so nothing is
lost. Unknown keys land in extra_json.

Runs one worker per discipline file.

Usage:
  python jsonl_to_parquet.py                 # full -> data/go-triple/parquet/
  python jsonl_to_parquet.py --limit 5000    # test: N lines per file
"""

import argparse
import glob
import gzip
import os
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow as pa
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DATA = os.path.join(HERE, "data", "go-triple")
OUT_DIR = os.path.join(DATA, "parquet")

SCHEMA = pa.schema([
    ("id", pa.string()),
    ("type", pa.string()),
    ("doi", pa.string()),            # first non-empty DOI — the join key
    ("title", pa.string()),          # first headline text
    ("discipline", pa.string()),     # from the filename
    ("primary_topic", pa.string()),  # highest-confidence topic id
    ("date_published", pa.string()),
    ("datestamp", pa.string()),
    ("language", pa.string()),       # first in_language
    ("provider", pa.string()),       # first provider
    ("publisher", pa.string()),      # first publisher
    ("url", pa.string()),            # first url
    ("is_cluster", pa.bool_()),
    ("is_duplicate", pa.bool_()),
    ("cluster_children_count", pa.int32()),
    ("n_authors", pa.int32()),
    # nested fields kept whole (column -> source key)
    ("doi_json", pa.string()),
    ("headline_json", pa.string()),
    ("abstract_json", pa.string()),
    ("author_json", pa.string()),
    ("contributor_json", pa.string()),
    ("keywords_json", pa.string()),
    ("knows_about_json", pa.string()),
    ("topic_json", pa.string()),
    ("provider_json", pa.string()),
    ("publisher_json", pa.string()),
    ("producer_json", pa.string()),
    ("in_language_json", pa.string()),
    ("license_json", pa.string()),
    ("conditions_of_access_json", pa.string()),
    ("identifier_json", pa.string()),
    ("url_json", pa.string()),
    ("main_entity_of_page_json", pa.string()),
    ("spatial_coverage_json", pa.string()),
    ("temporal_coverage_json", pa.string()),
    ("mentions_json", pa.string()),
    ("additional_type_json", pa.string()),
    ("original_languages_json", pa.string()),
    ("original_document_types_json", pa.string()),
    ("original_license_json", pa.string()),
    ("original_conditions_of_access_json", pa.string()),
    ("cluster_id_json", pa.string()),
    ("discarded_keywords_json", pa.string()),
    ("discarded_authors_json", pa.string()),
    ("extra_json", pa.string()),
])
COLUMNS = [f.name for f in SCHEMA]

JSON_KEYS = {
    "doi_json": "doi", "headline_json": "headline", "abstract_json": "abstract",
    "author_json": "author", "contributor_json": "contributor",
    "keywords_json": "keywords", "knows_about_json": "knows_about",
    "topic_json": "topic", "provider_json": "provider",
    "publisher_json": "publisher", "producer_json": "producer",
    "in_language_json": "in_language", "license_json": "license",
    "conditions_of_access_json": "conditions_of_access",
    "identifier_json": "identifier", "url_json": "url",
    "main_entity_of_page_json": "main_entity_of_page",
    "spatial_coverage_json": "spatial_coverage",
    "temporal_coverage_json": "temporal_coverage", "mentions_json": "mentions",
    "additional_type_json": "additional_type",
    "original_languages_json": "original_languages",
    "original_document_types_json": "original_document_types",
    "original_license_json": "original_license",
    "original_conditions_of_access_json": "original_conditions_of_access",
    "cluster_id_json": "cluster_id",
    "discarded_keywords_json": "discarded_keywords",
    "discarded_authors_json": "discarded_authors",
}
SCALAR_SRC = {"id", "type", "date_published", "datestamp", "is_cluster",
              "is_duplicate", "cluster_children_count", "full_text", "date_facets",
              "@id", "@type"}
KNOWN = set(JSON_KEYS.values()) | SCALAR_SRC | {"headline", "in_language", "url"}


def _dumps(v):
    return orjson.dumps(v).decode() if v else None


def _first_str(lst):
    for x in (lst or []):
        if isinstance(x, str) and x.strip():
            return x.strip()
    return None


def _first_label(lst):
    """First text from a CommonTranslatedLabel list, preferring the untranslated original."""
    if not lst:
        return None
    orig = [x for x in lst if isinstance(x, dict) and x.get("translated") in ("false", False)]
    for x in (orig + [y for y in lst if isinstance(y, dict)]):
        t = x.get("text")
        if t and t.strip():
            return t.strip()
    return None


def _primary_topic(lst):
    best, bc = None, -1.0
    for x in (lst or []):
        if isinstance(x, dict):
            c = x.get("confidence") or 0
            if c > bc:
                bc, best = c, x.get("id")
    return best


def parse_record(rec, discipline):
    row = {c: None for c in COLUMNS}
    row["discipline"] = discipline
    row["id"] = rec.get("id")
    row["type"] = rec.get("@type")
    row["doi"] = _first_str(rec.get("doi"))
    row["title"] = _first_label(rec.get("headline"))
    row["primary_topic"] = _primary_topic(rec.get("topic"))
    row["date_published"] = rec.get("date_published")
    row["datestamp"] = rec.get("datestamp")
    row["language"] = _first_str(rec.get("in_language"))
    row["provider"] = _first_str(rec.get("provider"))
    row["publisher"] = _first_str(rec.get("publisher"))
    row["url"] = _first_str(rec.get("url")) or _first_str(rec.get("main_entity_of_page"))
    row["is_cluster"] = rec.get("is_cluster")
    row["is_duplicate"] = rec.get("is_duplicate")
    cc = rec.get("cluster_children_count")
    row["cluster_children_count"] = cc if isinstance(cc, int) else None
    row["n_authors"] = len(rec.get("author") or [])
    for col, key in JSON_KEYS.items():
        row[col] = _dumps(rec.get(key))
    extra = {k: v for k, v in rec.items() if k not in KNOWN}
    row["extra_json"] = _dumps(extra)
    return row


def convert(args):
    gz_path, out_path, discipline, limit = args
    writer = pq.ParquetWriter(out_path, SCHEMA, compression="zstd", compression_level=3)
    cols = {c: [] for c in COLUMNS}
    n = 0
    pending = 0
    n_bad = 0

    def flush():
        nonlocal pending
        if pending:
            writer.write_table(pa.table(cols, schema=SCHEMA))
            for c in COLUMNS:
                cols[c].clear()
            pending = 0

    with gzip.open(gz_path, "rt", encoding="utf-8") as fh:
        for line in fh:
            if not line.strip():
                continue
            if limit and n >= limit:
                break
            try:
                row = parse_record(orjson.loads(line), discipline)
                for c in COLUMNS:
                    cols[c].append(row[c])
                pending += 1
                n += 1
                if pending >= 20000:
                    flush()
            except Exception:  # noqa: BLE001
                n_bad += 1
    flush()
    writer.close()
    return discipline, n, n_bad


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--in-dir", default=DATA)
    ap.add_argument("--out-dir", default=OUT_DIR)
    ap.add_argument("--workers", type=int, default=min(14, max(2, os.cpu_count() - 4)))
    ap.add_argument("--limit", type=int, default=None, help="test: N lines per file")
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    files = sorted(glob.glob(os.path.join(args.in_dir, "*_merged.jsonl.gz")))
    jobs = []
    for f in files:
        disc = os.path.basename(f)[: -len("_merged.jsonl.gz")]
        jobs.append((f, os.path.join(args.out_dir, f"{disc}.parquet"), disc, args.limit))
    print(f"{len(jobs)} discipline files -> Parquet with {args.workers} workers", flush=True)

    total = 0
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for disc, n, bad in pool.map(convert, jobs):
            total += n
            print(f"  {disc:14s} {n:>8,} rows  ({bad} bad)", flush=True)
    print(f"DONE: {total:,} rows across {len(jobs)} discipline files in {args.out_dir}", flush=True)


if __name__ == "__main__":
    main()
