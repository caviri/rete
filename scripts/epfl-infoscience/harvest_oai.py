"""Harvest the full EPFL Infoscience repository via OAI-PMH into JSONL.

Infoscience runs DSpace-CRIS; its OAI-PMH provider exposes one context,
`openaire4`, at https://infoscience.epfl.ch/server/oai/openaire4 with formats
oai_dc, oai_openaire and marcxml. All metadata is CC0. ~195,382 records.

We harvest ListRecords page-by-page following the resumptionToken (OAI's cursor
mechanism — no deep-pagination wall), parse each record's Dublin Core into a
flat JSON object, and append one JSON line per record.

- Resumable: the resumptionToken + written-record count are checkpointed after
  every page; a re-run truncates any partial tail and continues from the token.
- WAF-friendly: a browser User-Agent, a small delay between requests, and
  exponential backoff on HTTP errors or non-XML (WAF challenge) responses.
- Deleted records (OAI header status="deleted") are emitted with deleted=true.

Usage:
  python scripts/epfl-infoscience/harvest_oai.py            # full harvest (oai_dc)
  python scripts/epfl-infoscience/harvest_oai.py --format oai_openaire --out data/epfl-infoscience/infoscience_openaire.jsonl
  python scripts/epfl-infoscience/harvest_oai.py --max-pages 3 --fresh    # quick test
"""

import argparse
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

BASE = "https://infoscience.epfl.ch/server/oai/openaire4"
OUT = r"D:\pro\rete\data\epfl-infoscience\infoscience_oai_dc.jsonl"
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/125.0 Safari/537.36")
MAX_ATTEMPTS = 8

OAI = "{http://www.openarchives.org/OAI/2.0/}"
DC = "{http://purl.org/dc/elements/1.1/}"
OAI_DC = "{http://www.openarchives.org/OAI/2.0/oai_dc/}"
DC_FIELDS = ["title", "creator", "contributor", "subject", "description",
             "publisher", "date", "type", "format", "identifier", "language",
             "relation", "rights", "source", "coverage"]
DOI_RE = re.compile(r"10\.\d{4,9}/\S+")


def fetch(url):
    for attempt in range(1, MAX_ATTEMPTS + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA,
                                                       "Accept": "text/xml, application/xml"})
            with urllib.request.urlopen(req, timeout=90) as resp:
                data = resp.read()
            # WAF challenge / HTML landing → not the OAI XML we want
            if b"<?xml" not in data[:200] and b"<OAI-PMH" not in data[:2000]:
                raise ValueError("non-XML response (WAF challenge?)")
            return data
        except (urllib.error.HTTPError, urllib.error.URLError, ValueError,
                TimeoutError, OSError) as e:
            wait = min(2 ** attempt, 120)
            code = getattr(e, "code", "")
            print(f"    fetch retry {attempt}/{MAX_ATTEMPTS} ({code}{e}) waiting {wait}s", flush=True)
            time.sleep(wait)
    raise RuntimeError(f"giving up on {url}")


def parse_dc_record(rec):
    header = rec.find(f"{OAI}header")
    oai_id = header.findtext(f"{OAI}identifier")
    datestamp = header.findtext(f"{OAI}datestamp")
    sets = [s.text for s in header.findall(f"{OAI}setSpec")]
    deleted = header.get("status") == "deleted"
    row = {"oai_id": oai_id, "datestamp": datestamp, "sets": sets, "deleted": deleted}
    if deleted:
        return row
    dc = rec.find(f"{OAI}metadata/{OAI_DC}dc")
    vals = {f: [] for f in DC_FIELDS}
    if dc is not None:
        for el in dc:
            tag = el.tag.replace(DC, "")
            if tag in vals and el.text and el.text.strip():
                vals[tag].append(el.text.strip())
    row.update(vals)
    # derived join keys from identifiers
    doi = None
    urls, handle = [], None
    for ident in vals["identifier"]:
        m = DOI_RE.search(ident)
        if m and doi is None:
            doi = m.group(0).lower().rstrip(".")
        if ident.startswith("http"):
            urls.append(ident)
            hm = re.search(r"(20\.500\.\d+/\d+)", ident)
            if hm and handle is None:
                handle = hm.group(1)
    row["doi"] = doi
    row["handle"] = handle
    row["urls"] = urls
    return row


def load_ckpt(path):
    cp = path + ".ckpt.json"
    if os.path.exists(cp):
        with open(cp) as f:
            return json.load(f)
    return {"token": None, "count": 0, "page": 0, "done": False}


def save_ckpt(path, cp):
    tmp = path + ".ckpt.json.tmp"
    with open(tmp, "w") as f:
        json.dump(cp, f)
    os.replace(tmp, path + ".ckpt.json")


def truncate_to(path, n):
    """Keep only the first n lines (drop a partial tail from an interrupted run)."""
    if not os.path.exists(path):
        return
    kept = 0
    tmp = path + ".trunc"
    with open(path, encoding="utf-8") as fin, open(tmp, "w", encoding="utf-8") as fout:
        for line in fin:
            if kept >= n:
                break
            fout.write(line)
            kept += 1
    os.replace(tmp, path)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--format", default="oai_dc")
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--delay", type=float, default=0.4)
    ap.add_argument("--max-pages", type=int, default=None)
    ap.add_argument("--fresh", action="store_true")
    args = ap.parse_args()
    os.makedirs(os.path.dirname(args.out), exist_ok=True)

    cp = {"token": None, "count": 0, "page": 0, "done": False} if args.fresh else load_ckpt(args.out)
    if cp.get("done"):
        print(f"already complete: {cp['count']:,} records")
        return
    if args.fresh and os.path.exists(args.out):
        os.remove(args.out)
    if cp["count"]:
        print(f"resuming: {cp['count']:,} records, page {cp['page']}, truncating partial tail")
        truncate_to(args.out, cp["count"])

    total = None
    t0 = time.time()
    with open(args.out, "a", encoding="utf-8") as out:
        while True:
            if cp["token"]:
                url = f"{args.base}?verb=ListRecords&resumptionToken={urllib.parse.quote(cp['token'])}"
            else:
                url = f"{args.base}?verb=ListRecords&metadataPrefix={args.format}"
            root = ET.fromstring(fetch(url))
            err = root.find(f"{OAI}error")
            if err is not None:
                print(f"OAI error: {err.get('code')} {err.text}")
                break
            lr = root.find(f"{OAI}ListRecords")
            if lr is None:
                print("no ListRecords element; stopping")
                break
            n = 0
            for rec in lr.findall(f"{OAI}record"):
                out.write(json.dumps(parse_dc_record(rec), ensure_ascii=False) + "\n")
                n += 1
            out.flush()
            cp["count"] += n
            cp["page"] += 1
            rt = lr.find(f"{OAI}resumptionToken")
            if total is None and rt is not None and rt.get("completeListSize"):
                total = int(rt.get("completeListSize"))
            cp["token"] = rt.text.strip() if (rt is not None and rt.text and rt.text.strip()) else None
            save_ckpt(args.out, cp)
            rate = cp["count"] / max(time.time() - t0, 1)
            pct = f"{100*cp['count']/total:.1f}%" if total else "?"
            print(f"page {cp['page']:>5} | {cp['count']:>7,}/{total or '?'} ({pct}) | "
                  f"{rate:.0f} rec/s", flush=True)
            if cp["token"] is None:
                cp["done"] = True
                save_ckpt(args.out, cp)
                break
            if args.max_pages and cp["page"] >= args.max_pages:
                print("reached --max-pages")
                break
            time.sleep(args.delay)

    print(f"DONE: {cp['count']:,} records in {(time.time()-t0)/60:.1f} min -> {args.out}")


if __name__ == "__main__":
    main()
