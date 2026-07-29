#!/usr/bin/env python3
"""Profile the harvested WikiArt corpus: coverage, fill rates, distributions.

    MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w \
      python:3.12-slim python data/wikiart/scripts/inspect.py

Prints the numbers that go in the README and that the graph modelling depends
on: how complete each layer is, which fields are worth projecting, and how the
two id systems line up.
"""

import json
import os
import re
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import raw_dir     # noqa: E402

RAW = raw_dir()
ASPNET_DATE = re.compile(r"^/Date\((-?\d+)\)/$")


def rd(name):
    p = os.path.join(RAW, name)
    if not os.path.exists(p):
        return []
    out = []
    for line in open(p, encoding="utf-8"):
        line = line.strip()
        if line:
            try:
                out.append(json.loads(line))
            except Exception:
                pass
    return out


def h(t):
    print(f"\n{t}\n{'-' * len(t)}")


def fill(recs, label, top=None):
    """Report per-field fill rate, and value distribution for small vocabularies."""
    if not recs:
        print(f"  (no {label})")
        return
    n = len(recs)
    keys = Counter()
    for r in recs:
        for k, v in r.items():
            if v not in (None, "", [], {}):
                keys[k] += 1
    print(f"  {n:,} records, {len(keys)} populated fields")
    for k, c in sorted(keys.items(), key=lambda x: -x[1]):
        print(f"    {k:<24} {c:>9,}  {100.0*c/n:5.1f}%")
    for k in (top or []):
        vals = Counter()
        for r in recs:
            v = r.get(k)
            if isinstance(v, list):
                vals.update(str(x) for x in v)
            elif v not in (None, ""):
                vals[str(v)] += 1
        if vals:
            print(f"    top {k}: " + ", ".join(f"{v} ({c:,})" for v, c in vals.most_common(8)))


def main():
    print(f"WikiArt raw corpus at {RAW}")

    # -- inventory ground truth ----------------------------------------------
    h("Sitemap inventory (ground truth)")
    sm = os.path.join(RAW, "sitemap")
    totals = {}
    if os.path.isdir(sm):
        for f in sorted(os.listdir(sm)):
            if f.endswith(".xml") and f != "sitemap_index.xml":
                c = open(os.path.join(sm, f), encoding="utf-8", errors="replace").read().count("<loc>")
                totals[f] = c
                print(f"  {f:<34} {c:>8,}")
    declared = sum(v for k, v in totals.items() if k.startswith("paintings-"))
    print(f"  => {declared:,} painting URLs declared by WikiArt")

    # -- vocabularies ---------------------------------------------------------
    h("Dictionaries (controlled vocabularies)")
    ddir = os.path.join(RAW, "dictionaries")
    if os.path.isdir(ddir):
        tot = 0
        for f in sorted(os.listdir(ddir)):
            if f.startswith("group-"):
                n = len(json.load(open(os.path.join(ddir, f), encoding="utf-8")))
                tot += n
                print(f"  {f[:-5]:<34} {n:>6,}")
        print(f"  => {tot:,} vocabulary entries")

    # -- artists --------------------------------------------------------------
    h("Artists")
    alpha_p = os.path.join(RAW, "artists_alphabet.json")
    alpha = json.load(open(alpha_p, encoding="utf-8")) if os.path.exists(alpha_p) else []
    rich, recov = rd("artists.jsonl"), rd("artists_recovered.jsonl")
    print(f"  complete slug layer      {len(alpha):,}")
    print(f"  rich v2 layer            {len(rich):,}")
    print(f"  recovered Mongo ids      {len(recov):,}")
    known = {a.get("url") for a in rich} | {a.get("url") for a in recov}
    if alpha:
        cov = sum(1 for a in alpha if a.get("url") in known)
        print(f"  Mongo-id coverage        {cov:,}/{len(alpha):,} ({100.0*cov/len(alpha):.1f}%)")
    if rich:
        fill(rich, "rich artists", top=["gender"])

    # -- paintings ------------------------------------------------------------
    h("Paintings -- inventory (App layer, contentId)")
    app = rd("paintings_app.jsonl")
    if app:
        print(f"  {len(app):,} of {declared:,} declared "
              f"({100.0*len(app)/max(declared,1):.1f}% of the sitemap)")
        yrs = [r.get("completitionYear") for r in app if isinstance(r.get("completitionYear"), int)]
        if yrs:
            print(f"  completion years: {min(yrs)} .. {max(yrs)}")
        fill(app, "app paintings")

    h("Paintings -- detail (App/ImageJson: dictionaries[], market data)")
    fill(rd("paintings_imagejson.jsonl"), "imagejson paintings",
         top=["style", "genre", "galleryName"])

    h("Paintings -- detail (v2: description, tags[], galleries[])")
    v2 = rd("paintings.jsonl")
    fill(v2, "v2 paintings", top=["styles", "genres", "media"])
    if v2:
        desc = [r for r in v2 if r.get("description")]
        print(f"  {len(desc):,} carry curatorial prose "
              f"({100.0*len(desc)/len(v2):.1f}%), "
              f"mean {sum(len(r['description']) for r in desc)//max(len(desc),1):,} chars")
        links = sum(len(re.findall(r"\[url href=", r["description"])) for r in desc)
        print(f"  {links:,} [url] cross-references embedded in descriptions "
              f"(-> citation edges between WikiArt entities)")

    # -- id join --------------------------------------------------------------
    h("Joining the two id systems on (artistUrl, url)")
    if app and v2:
        a_keys = {(r.get("_artistUrl"), r.get("url")) for r in rd("paintings_imagejson.jsonl")}
        v_keys = {(r.get("artistUrl"), r.get("url")) for r in v2}
        if a_keys and v_keys:
            inter = a_keys & v_keys
            print(f"  ImageJson keys {len(a_keys):,} | v2 keys {len(v_keys):,} | overlap {len(inter):,}")

    # -- images ---------------------------------------------------------------
    h("Image assets")
    man = os.path.join(RAW, "assets", "images.urls.txt")
    if os.path.exists(man):
        n = sum(1 for _ in open(man, encoding="utf-8"))
        print(f"  {n:,} image URLs in the manifest (bytes not downloaded by default)")
    else:
        print("  (manifest not built yet -- run extract_image_urls.py)")


if __name__ == "__main__":
    main()
