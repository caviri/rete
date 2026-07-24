#!/usr/bin/env python3
"""Profile the harvested ETH Research Collection oai_ethz pages.

Reads data/ethz-research-collection/raw/oai_ethz/page_*.xml.gz and reports:
  record count, field fill rates, publication-type distribution, license /
  availability breakdown, and key-identifier coverage (DOI/arXiv/WoS/…).

Stdlib only; run in Docker:
  docker run --rm -v "$PWD:/w" -w //w python:3.12-slim \
    python data/ethz-research-collection/scripts/inspect.py
"""

import gzip
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

OAI = "{http://www.openarchives.org/OAI/2.0/}"
BASE = Path("data/ethz-research-collection/raw")
RAW = BASE / "oai_ethz"
XOAI = BASE / "xoai"


def local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1] if "}" in tag else tag


def text_of(el) -> str:
    return (el.text or "").strip()


def profile_xoai() -> None:
    """Profile the complete xoai dump: entity types, relationship graph, files."""
    pages = sorted(XOAI.glob("page_*.xml.gz"))
    if not pages:
        print(f"\n(no xoai pages in {XOAI} yet — skipping enrichment profile)")
        return

    def named_children(el, name):
        for c in el:
            if c.tag.split("}")[-1] == "element" and c.get("name") == name:
                yield c

    def first_named(el, name):
        return next(named_children(el, name), None)

    n = 0
    entity_types = Counter()
    rel_types = Counter()          # isJournalOfPublication -> #edges
    items_with_rel = 0
    items_with_files = 0
    bundle_names = Counter()
    mime_types = Counter()
    n_bitstreams = 0
    total_bytes = 0

    for p in pages:
        with gzip.open(p, "rb") as f:
            root = ET.parse(f).getroot()
        for rec in root.iter(f"{OAI}record"):
            md = rec.find(f"{OAI}metadata")
            if md is None:
                continue
            n += 1
            inner = next((e for e in md if e.tag.split("}")[-1] == "metadata"), None)
            if inner is None:
                continue
            # entity type: dspace.entity.type
            dspace = first_named(inner, "dspace")
            if dspace is not None:
                ent = first_named(dspace, "entity")
                et = first_named(ent, "type") if ent is not None else None
                if et is not None:
                    for fld in et.iter():
                        if fld.tag.split("}")[-1] == "field" and (fld.text or "").strip():
                            entity_types[fld.text.strip()] += 1
            # relationships: <relation><isX>...<value>UUID
            rel = first_named(inner, "relation")
            if rel is not None:
                had = False
                for rt in rel:
                    if rt.tag.split("}")[-1] != "element":
                        continue
                    name = rt.get("name")
                    if name == "latestForDiscovery":
                        continue
                    cnt = sum(1 for none in named_children(rt, "none")
                              for fld in none if fld.tag.split("}")[-1] == "field"
                              and fld.get("name") == "value")
                    if cnt:
                        rel_types[name] += cnt
                        had = True
                items_with_rel += 1 if had else 0
            # files: <bundles><bundle><bitstreams><bitstream>
            bundles = first_named(inner, "bundles")
            if bundles is not None:
                has_file = False
                for bundle in named_children(bundles, "bundle"):
                    bn = first_named(bundle, "name")
                    if bn is not None:
                        for fld in bn.iter():
                            if fld.tag.split("}")[-1] == "field" and (fld.text or "").strip():
                                bundle_names[fld.text.strip()] += 1
                    bs = first_named(bundle, "bitstreams")
                    if bs is None:
                        continue
                    for bit in named_children(bs, "bitstream"):
                        has_file = True
                        n_bitstreams += 1
                        for fld in bit:
                            if fld.tag.split("}")[-1] != "field":
                                continue
                            key, val = fld.get("name"), (fld.text or "").strip()
                            if key == "format" and val:
                                mime_types[val] += 1
                            elif key == "size" and val.isdigit():
                                total_bytes += int(val)
                if has_file:
                    items_with_files += 1

    def pct(x):
        return f"{100 * x / n:5.1f}%" if n else "  n/a"

    print(f"\n=== xoai — complete-dump enrichment profile ===")
    print(f"records: {n:,}   ({len(pages)} pages)")
    print(f"\n--- entity types (dspace.entity.type) ---")
    for t, c in entity_types.most_common():
        print(f"  {c:>7,}  {pct(c)}  {t}")
    print(f"\n--- relationship edges by type ---")
    print(f"  records with >=1 relationship: {items_with_rel:,} ({pct(items_with_rel)})")
    for t, c in rel_types.most_common():
        print(f"  {c:>7,}  {t}")
    print(f"\n--- files / bitstreams ---")
    print(f"  records with >=1 file: {items_with_files:,} ({pct(items_with_files)})")
    print(f"  bitstreams total:      {n_bitstreams:,}")
    print(f"  bitstream bytes total: {total_bytes:,}  ({total_bytes/1e12:.2f} TB, not downloaded)")
    print(f"  bundles:")
    for b, c in bundle_names.most_common():
        print(f"    {c:>7,}  {b}")
    print(f"  MIME types (top 12):")
    for m, c in mime_types.most_common(12):
        print(f"    {c:>7,}  {m}")


def main() -> None:
    pages = sorted(RAW.glob("page_*.xml.gz"))
    if not pages:
        raise SystemExit(f"no pages in {RAW} — run download.sh first")

    n_records = 0
    n_deleted = 0
    field_fill = Counter()      # localname -> #records having it >=1
    field_values = Counter()    # localname -> total value count
    types = Counter()
    licenses = Counter()
    availability = Counter()
    languages = Counter()
    id_coverage = Counter()     # doi/arxiv/wos/scopus/issn/isbn/pmid/handle
    year_hist = Counter()
    set_hist = Counter()
    author_counts = []

    ID_MAP = {
        "identifier-doi": "doi",
        "identifier-arxiv": "arxiv",
        "identifier-wos": "wos",
        "identifier-scopus": "scopus",
        "identifier-issn": "issn",
        "identifier-isbn": "isbn",
        "identifier-pmid": "pmid",
        "identifier-uri": "handle_uri",
    }

    for p in pages:
        with gzip.open(p, "rb") as f:
            root = ET.parse(f).getroot()
        for rec in root.iter(f"{OAI}record"):
            n_records += 1
            header = rec.find(f"{OAI}header")
            if header is not None and header.get("status") == "deleted":
                n_deleted += 1
            for s in rec.findall(f"{OAI}header/{OAI}setSpec"):
                set_hist[text_of(s)] += 1
            md = rec.find(f"{OAI}metadata")
            if md is None:
                continue
            seen = set()
            n_authors = 0
            for el in md.iter():
                name = local(el.tag)
                if name in ("dc", "metadata"):
                    continue
                field_values[name] += 1
                if name not in seen:
                    field_fill[name] += 1
                    seen.add(name)
                val = text_of(el)
                if name == "type" and val:
                    types[val] += 1
                elif name == "rights-license" and val:
                    licenses[val] += 1
                elif name == "availability" and val:
                    availability[val] += 1
                elif name == "language-iso" and val:
                    languages[val] += 1
                elif name == "contributor-author-name" and val:
                    n_authors += 1
                elif name == "date-issued" and val:
                    year_hist[val[:4]] += 1
                for field, key in ID_MAP.items():
                    if name == field and val:
                        id_coverage[key] += 1
            author_counts.append(n_authors)

    def pct(x):
        return f"{100 * x / n_records:5.1f}%" if n_records else "  n/a"

    print(f"\n=== ETH Research Collection — oai_ethz profile ===")
    print(f"pages:   {len(pages)}")
    print(f"records: {n_records:,}  (deleted: {n_deleted:,})")

    print(f"\n--- publication types (top 25) ---")
    for t, c in types.most_common(25):
        print(f"  {c:>7,}  {pct(c)}  {t}")

    print(f"\n--- identifier coverage ---")
    for k in ["doi", "arxiv", "wos", "scopus", "issn", "isbn", "pmid", "handle_uri"]:
        print(f"  {id_coverage[k]:>7,}  {pct(id_coverage[k])}  {k}")

    print(f"\n--- availability ---")
    for a, c in availability.most_common():
        print(f"  {c:>7,}  {pct(c)}  {a}")

    print(f"\n--- licenses (top 15) ---")
    for lic, c in licenses.most_common(15):
        print(f"  {c:>7,}  {pct(c)}  {lic}")

    print(f"\n--- languages (top 10) ---")
    for lang, c in languages.most_common(10):
        print(f"  {c:>7,}  {pct(c)}  {lang}")

    print(f"\n--- issue year (recent 15) ---")
    for y in sorted((y for y in year_hist if y.isdigit()), reverse=True)[:15]:
        print(f"  {year_hist[y]:>7,}  {y}")

    if author_counts:
        nz = [a for a in author_counts if a]
        print(f"\n--- authors/record ---")
        print(f"  records with >=1 author: {len(nz):,} ({pct(len(nz))})")
        print(f"  mean (nonzero): {sum(nz)/len(nz):.1f}   max: {max(author_counts)}")

    print(f"\n--- top-level sets (setSpec, top 20) ---")
    for s, c in set_hist.most_common(20):
        print(f"  {c:>7,}  {s}")

    print(f"\n--- field fill rates (top 60 by presence) ---")
    for name, c in field_fill.most_common(60):
        avg = field_values[name] / c if c else 0
        print(f"  {pct(c)}  {c:>7,}  {name}  (avg {avg:.1f}/rec)")

    profile_xoai()


if __name__ == "__main__":
    main()
