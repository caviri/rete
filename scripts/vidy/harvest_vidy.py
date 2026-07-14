#!/usr/bin/env python3
"""Harvest the Archives de la Ville de Lausanne (AtoM 2.8.2, vidy-archives.lausanne.ch).

OAI-PMH is disabled here, so the structured route is per-fonds EAD export
(`/index.php/<slug>;ead?sf_format=xml`) behind a trivial static-cookie JS challenge
(`js_verified=1`). A fonds-level EAD nests its whole ISAD(G) subtree, so ~541 depth-1
fonds exports cover the ~39,732 archival descriptions. Each <c> component carries its
cote, title, dates, physdesc, physical container, and its master <dao> PDF link.

Then HEAD every PDF for its exact byte size (the digital objects are big — ~100 MB each).
Output: data/vidy/records.jsonl (one archival unit per line, with pdf_url + pdf_size).

Polite: sequential exports, browser UA, retries, resumable (skips done fonds; caches
raw EADs under data/vidy/ead/). robots disallows sf_format=xml — run under the owner's
explicit authorization for this open city archive.
"""
import os, re, sys, json, gzip, time, ssl, urllib.request
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

B   = "https://vidy-archives.lausanne.ch"
UA  = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/125.0 Safari/537.36"
HDR = {"User-Agent": UA, "Cookie": "js_verified=1"}
REPO = Path(__file__).resolve().parents[2]
OUT  = REPO / "data" / "vidy"
EADD = OUT / "ead"
OUT.mkdir(parents=True, exist_ok=True); EADD.mkdir(exist_ok=True)
CTX = ssl.create_default_context(); CTX.check_hostname = False; CTX.verify_mode = ssl.CERT_NONE
COLL = {"adm": "Archives administratives", "anc": "Archives anciennes", "pri": "Archives privées",
        "bib": "Bibliothèque", "doc": "Documentation", "h": "H"}

def fetch(url, timeout=200):
    last = None
    for a in range(3):
        try:
            return urllib.request.urlopen(urllib.request.Request(url, headers=HDR), timeout=timeout, context=CTX).read()
        except Exception as e:
            last = e; time.sleep(2 * (a + 1))
    raise last

def T(e): return e.tag.split("}")[-1]
def gettext(e, path):
    x = e.find(path)
    return re.sub(r"\s+", " ", (x.text or "").strip()) if (x is not None and x.text) else ""

def sitemap_slugs():
    raw = gzip.decompress(fetch(f"{B}/sitemap.1.xml.gz"))
    slugs = [re.sub(r"^https?://[^/]+/", "", u) for u in re.findall(r"<loc>([^<]+)</loc>", raw.decode("utf-8", "replace"))]
    return [s for s in slugs if s and "sitemap" not in s]

def fonds_list(slugs):
    fonds = []
    for s in slugs:
        m = re.match(r"^(adm|anc|pri|bib|doc|h)-([^-]+)$", s)   # depth-1 = collection-<one segment>
        if m:
            fonds.append(s)
    return sorted(set(fonds))

def parse_ead(xml_bytes, records):
    """Add every archdesc/c component in this EAD to records{cote|title: rec}."""
    try:
        root = ET.fromstring(xml_bytes)
    except ET.ParseError:
        return 0
    added = 0
    def collection_of(cote):
        p = (cote or "").split("-", 1)[0].lower()
        return COLL.get(p, p)
    def walk(node, parent_cote):
        nonlocal added
        did = node.find("{*}did")
        if did is None:
            for ch in node:
                if T(ch) in ("c", "dsc"):
                    walk(ch, parent_cote)
            return
        cote = gettext(did, "{*}unitid")
        title = gettext(did, "{*}unittitle")
        key = cote or title
        if key and key not in records:
            cont = did.find("{*}container")
            dao = did.find("{*}dao")
            rep = node.find(".//{*}repository/{*}corpname")
            orig = node.find(".//{*}origination")
            pub = None
            for odd in node.findall("{*}odd"):
                if odd.get("type") == "publicationStatus":
                    pub = gettext(odd, "{*}p")
            records[key] = {
                "cote": cote,
                "slug": cote.lower() if cote else None,
                "title": title,
                "level": node.get("level"),
                "date": gettext(did, "{*}unitdate"),
                "physdesc": gettext(did, "{*}physdesc"),
                "container": (re.sub(r"\s+", " ", cont.text.strip()) if (cont is not None and cont.text) else ""),
                "container_type": (cont.get("type") if cont is not None else ""),
                "repository": (re.sub(r"\s+", " ", rep.text.strip()) if (rep is not None and rep.text) else ""),
                "producer": (re.sub(r"\s+", " ", "".join(orig.itertext()).strip()) if orig is not None else ""),
                "publication_status": pub,
                "parent_cote": parent_cote,
                "collection": collection_of(cote),
                "pdf_url": (dao.get("href") if dao is not None else None),
                "pdf_size": None,
            }
            added += 1
        # recurse
        for ch in node:
            if T(ch) in ("dsc", "c"):
                walk(ch, cote or parent_cote)
    arch = root.find(".//{*}archdesc")
    if arch is not None:
        walk(arch, None)
    return added

def main():
    print("fetching sitemap…", flush=True)
    slugs = sitemap_slugs()
    fonds = fonds_list(slugs)
    info_slugs = set(s for s in slugs if re.match(r"^(adm|anc|pri|bib|doc|h)(-|$)", s))
    print(f"{len(slugs):,} slugs · {len(info_slugs):,} info-objects · {len(fonds)} depth-1 fonds to export", flush=True)

    records = {}
    done_file = OUT / "done_fonds.txt"
    done = set(done_file.read_text().split()) if done_file.exists() else set()
    # reload any previously-saved records
    rec_file = OUT / "records.jsonl"
    if rec_file.exists():
        for line in rec_file.open(encoding="utf-8"):
            r = json.loads(line); records[r["cote"] or r["title"]] = r

    for i, f in enumerate(fonds, 1):
        if f in done:
            continue
        cache = EADD / f"{f}.xml"
        try:
            xml = cache.read_bytes() if cache.exists() else fetch(f"{B}/index.php/{f};ead?sf_format=xml")
            if not cache.exists():
                cache.write_bytes(xml)
            n = parse_ead(xml, records)
            print(f"[{i}/{len(fonds)}] {f}: +{n} (total {len(records):,})", flush=True)
        except Exception as e:
            print(f"[{i}/{len(fonds)}] {f}: ERR {e}", flush=True)
            continue
        done.add(f)
        if i % 10 == 0:
            done_file.write_text(" ".join(sorted(done)))
            with rec_file.open("w", encoding="utf-8") as out:
                for r in records.values():
                    out.write(json.dumps(r, ensure_ascii=False) + "\n")
    done_file.write_text(" ".join(sorted(done)))

    # backfill missing info-slugs shallow-first: a depth-2 sub-series export pulls in its
    # whole subtree, so after the depth-2 wave only the leaves under a 504-timeout series
    # (e.g. adm-f-6's ~25k plans) remain, fetched individually. Network parallelised;
    # parsing stays single-threaded (records dict isn't thread-safe). Resumable via cache.
    def captured():
        return set(r["slug"] for r in records.values() if r.get("slug"))
    def fetch_ead(s):
        cache = EADD / f"bf_{re.sub(r'[^A-Za-z0-9._-]', '_', s)}.xml"
        try:
            if cache.exists():
                return (s, cache.read_bytes())
            xml = fetch(f"{B}/index.php/{s};ead?sf_format=xml", timeout=120)
            cache.write_bytes(xml)
            return (s, xml)
        except Exception:
            return (s, None)
    for depth in sorted(set(s.count("-") for s in info_slugs)):
        have = captured()
        wave = [s for s in info_slugs if s.count("-") == depth and s not in have]
        if not wave:
            continue
        print(f"backfill depth {depth}: {len(wave):,} slugs…", flush=True)
        k = 0
        with ThreadPoolExecutor(max_workers=12) as ex:
            for s, xml in ex.map(fetch_ead, wave):
                if xml:
                    parse_ead(xml, records)
                k += 1
                if k % 500 == 0:
                    print(f"  depth {depth}: {k}/{len(wave)} (total {len(records):,})", flush=True)
        with rec_file.open("w", encoding="utf-8") as out:
            for r in records.values():
                out.write(json.dumps(r, ensure_ascii=False) + "\n")

    # phase 2: HEAD every PDF for its exact byte size
    pdfs = sorted(set(r["pdf_url"] for r in records.values() if r.get("pdf_url")))
    print(f"\n{len(records):,} records; {len(pdfs):,} unique PDFs → HEAD for sizes…", flush=True)
    def head(u):
        try:
            r = urllib.request.urlopen(urllib.request.Request(u, headers=HDR, method="HEAD"), timeout=90, context=CTX)
            return (u, int(r.headers.get("Content-Length") or 0), (r.headers.get("Content-Type") or "").split(";")[0])
        except Exception:
            return (u, None, None)
    sizes = {}
    with ThreadPoolExecutor(max_workers=8) as ex:
        for k, (u, sz, ct) in enumerate(ex.map(head, pdfs), 1):
            sizes[u] = sz
            if k % 500 == 0:
                print(f"  HEAD {k}/{len(pdfs)}", flush=True)
    total = 0
    for r in records.values():
        if r.get("pdf_url"):
            r["pdf_size"] = sizes.get(r["pdf_url"])
            if r["pdf_size"]:
                total += r["pdf_size"]

    with rec_file.open("w", encoding="utf-8") as out:
        for r in records.values():
            out.write(json.dumps(r, ensure_ascii=False) + "\n")
    with_pdf = sum(1 for r in records.values() if r.get("pdf_url"))
    print(f"\nDONE: {len(records):,} records → {rec_file}", flush=True)
    print(f"  with a PDF: {with_pdf:,}  ·  total PDF bytes: {total/1024/1024/1024:.1f} GB", flush=True)

if __name__ == "__main__":
    main()
