#!/usr/bin/env python3
"""Exhaustive field census across every raw WikiArt file.

This is step 0 of the ontology work: enumerate what the data ACTUALLY contains,
so the ontology can be checked for total coverage rather than assumed complete.
For every field of every entity it reports fill rate, observed JSON types,
distinct-value count, numeric range, and sample values -- including fields
nested inside lists/objects.

    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      -e PYTHONIOENCODING=utf-8 python:3.12-slim \
      python data/wikiart/scripts/field_census.py > data/wikiart/FIELDS.md

Writes Markdown so the result is reviewable and diffable.
"""

import csv
import json
import os
import sys
from collections import Counter, defaultdict

RAW = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "raw"))
MAX_SAMPLES = 3
SAMPLE_CAP = 60          # chars per sample value


def jtype(v):
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int"
    if isinstance(v, float):
        return "float"
    if isinstance(v, str):
        return "string"
    if isinstance(v, list):
        return "list"
    if isinstance(v, dict):
        return "object"
    return type(v).__name__


class Field:
    __slots__ = ("present", "nonempty", "types", "distinct", "samples", "nmin", "nmax",
                 "lmin", "lmax")

    def __init__(self):
        self.present = 0
        self.nonempty = 0
        self.types = Counter()
        self.distinct = set()
        self.samples = []
        self.nmin = self.nmax = None
        self.lmin = self.lmax = None

    def add(self, v):
        self.present += 1
        self.types[jtype(v)] += 1
        if v in (None, "", [], {}):
            return
        self.nonempty += 1
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            self.nmin = v if self.nmin is None else min(self.nmin, v)
            self.nmax = v if self.nmax is None else max(self.nmax, v)
        if isinstance(v, (list, dict)):
            n = len(v)
            self.lmin = n if self.lmin is None else min(self.lmin, n)
            self.lmax = n if self.lmax is None else max(self.lmax, n)
        key = json.dumps(v, ensure_ascii=False, sort_keys=True) if isinstance(v, (list, dict)) else v
        if len(self.distinct) < 200_000:
            self.distinct.add(str(key)[:200])
        if len(self.samples) < MAX_SAMPLES:
            s = str(key).replace("\n", " ").replace("\t", " ")
            if len(s) > SAMPLE_CAP:
                s = s[:SAMPLE_CAP] + "…"
            if s not in self.samples:
                self.samples.append(s)


def walk(obj, fields, prefix=""):
    """Record every leaf/branch path. Lists recurse into their element objects."""
    if isinstance(obj, dict):
        for k, v in obj.items():
            path = f"{prefix}{k}"
            fields[path].add(v)
            if isinstance(v, dict):
                walk(v, fields, path + ".")
            elif isinstance(v, list) and v and isinstance(v[0], dict):
                for item in v[:50]:
                    if isinstance(item, dict):
                        walk(item, fields, path + "[].")


def report(title, n, fields, note=""):
    print(f"\n## {title}\n")
    print(f"`{n:,}` records{('  — ' + note) if note else ''}\n")
    print("| field | fill | types | distinct | range / size | samples |")
    print("|---|---|---|---|---|---|")
    for name, f in sorted(fields.items(), key=lambda kv: (-kv[1].nonempty, kv[0])):
        pct = 100.0 * f.nonempty / n if n else 0
        types = ", ".join(f"{t}" for t, _ in f.types.most_common() if t != "null")
        d = len(f.distinct)
        dtxt = f"{d:,}" + ("+" if d >= 200_000 else "")
        rng = ""
        if f.nmin is not None:
            rng = (f"{f.nmin:g} … {f.nmax:g}")
        elif f.lmin is not None:
            rng = f"len {f.lmin}–{f.lmax}"
        smp = " · ".join(f"`{s}`" for s in f.samples)
        print(f"| `{name}` | {pct:.1f}% | {types} | {dtxt} | {rng} | {smp} |")


def census_jsonl(path, title, note=""):
    p = os.path.join(RAW, path)
    if not os.path.exists(p):
        print(f"\n## {title}\n\n_missing: {path}_")
        return
    fields = defaultdict(Field)
    n = 0
    for line in open(p, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except Exception:
            continue
        n += 1
        walk(rec, fields)
    report(title, n, fields, note)


def census_json_array(path, title, note="", key=None):
    p = os.path.join(RAW, path)
    if not os.path.exists(p):
        print(f"\n## {title}\n\n_missing: {path}_")
        return
    doc = json.load(open(p, encoding="utf-8"))
    recs = doc if isinstance(doc, list) else (doc.get(key) or [])
    fields = defaultdict(Field)
    for rec in recs:
        if isinstance(rec, dict):
            walk(rec, fields)
    report(title, len(recs), fields, note)


def census_tsv(path, title, note=""):
    p = os.path.join(RAW, path)
    if not os.path.exists(p):
        print(f"\n## {title}\n\n_missing: {path}_")
        return
    fields = defaultdict(Field)
    n = 0
    with open(p, encoding="utf-8") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            n += 1
            for k, v in row.items():
                if k is None:
                    continue
                fields[k].add(v if v not in ("", None) else None)
    report(title, n, fields, note)


def census_dictionaries():
    d = os.path.join(RAW, "dictionaries")
    if not os.path.isdir(d):
        return
    print("\n## Dictionaries (v2 controlled vocabularies)\n")
    print("| group file | entries | fields |")
    print("|---|---|---|")
    allf = defaultdict(Field)
    tot = 0
    for fn in sorted(os.listdir(d)):
        if not fn.startswith("group-"):
            continue
        recs = json.load(open(os.path.join(d, fn), encoding="utf-8"))
        tot += len(recs)
        ks = set()
        for r in recs:
            ks.update(r.keys())
            walk(r, allf)
        print(f"| `{fn}` | {len(recs):,} | {', '.join(sorted(ks))} |")
    print(f"\n**{tot:,} entries total.** Union of fields across all groups:\n")
    report("Dictionary entry fields", tot, allf)


def census_categories():
    d = os.path.join(RAW, "categories")
    if not os.path.isdir(d):
        return
    ent = defaultdict(Field)
    cat = defaultdict(Field)
    ne = nc = 0
    langs = set()
    print("\n## Facet vocabularies (`/en/artists-by-<facet>?json=2`)\n")
    print("| facet | group | entries | categories |")
    print("|---|---|---|---|")
    for fn in sorted(os.listdir(d)):
        if not fn.endswith(".json"):
            continue
        doc = json.load(open(os.path.join(d, fn), encoding="utf-8"))
        es, cs = doc.get("entries") or [], doc.get("categories") or []
        print(f"| `{fn[:-5]}` | {doc.get('group')} | {len(es):,} | {len(cs)} |")
        for e in es:
            ne += 1
            walk(e, ent)
        for c in cs:
            nc += 1
            walk(c, cat)
            t = (((c.get("Content") or {}).get("Title") or {}).get("Title")) or {}
            langs.update(t.keys())
    report("Facet entry fields", ne, ent)
    report("Facet category fields", nc, cat)
    print(f"\n**Category titles carry {len(langs)} languages:** "
          f"{', '.join(sorted(langs))}\n")


def main():
    print("# WikiArt raw field census")
    print("\n_Generated by `scripts/field_census.py` from the actual data "
          "(not from any documentation). Fill % is of non-empty values._")

    census_jsonl("paintings_imagejson.jsonl", "Paintings — detail (App `ImageJson`)",
                 "the primary artwork record; COMPLETE coverage")
    census_jsonl("paintings_app.jsonl", "Paintings — inventory (App `PaintingsByArtist`)",
                 "oeuvre listing; source of `contentId`")
    census_jsonl("artists.jsonl", "Artists — rich (v2 `UpdatedArtists`)",
                 "5,100 of 5,755; the metered layer")
    census_json_array("artists_alphabet.json", "Artists — complete (App `AlphabetJson`)",
                      "all 5,755; numeric ids")
    census_jsonl("artists_recovered.jsonl", "Artists — recovered Mongo ids", "")
    census_dictionaries()
    census_categories()
    census_tsv("assets/webp_manifest.tsv", "Image mirror manifest", "local WebP derivatives")

    print("\n---\n")
    print("Every field above must appear in the ontology or be explicitly "
          "listed as deliberately dropped. See `wikiart.ttl` coverage table.")


if __name__ == "__main__":
    main()
