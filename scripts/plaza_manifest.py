#!/usr/bin/env python3
"""Generate docs/plaza/plaza.json from the playground catalog.

    python scripts/plaza_manifest.py                 # derive, probe every URL, write
    python scripts/plaza_manifest.py --no-probe      # derive only (offline), write
    python scripts/plaza_manifest.py --check         # offline: fail if plaza.json is stale
    python scripts/plaza_manifest.py --check --probe # also re-probe every URL

Plain Python 3.12 stdlib, runs on the host. Inputs:

  web/playground-src/catalog.js   the published catalog. It is JavaScript
                                  (window.RETE_PLAYGROUND_CATALOG = {...}, with
                                  comments, a const above it and trailing commas),
                                  so it is evaluated with `node` rather than
                                  parsed in Python.
  web/datasets.lock.json          sizes/format for the few pinned datasets
  docs/plaza/plaza.curated.json   hand-curated additions: icon, tags, blurb,
                                  companions, links, connections, ... per key,
                                  plus the manifest's top-level fields. Curated
                                  fields win over derived ones.

Every URL the manifest points to (the .rete, its shards, pmtiles and companion
files) is checked with a ranged GET of bytes 0-1023 (8 at a time). A dataset
whose .rete answers 404/403/410 or is not a .rete is dropped and listed under
"_dropped"; a companion that 404s is dropped from its entry. Anything else
(timeouts, 5xx after retries) aborts the run: a flaky network must never
silently shrink the plaza. Local bundled files are read from disk, not probed.

Probe-derived fields (formatVersion, size, verified) move whenever a file is
republished, so --check ignores them unless --probe is given.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import re
import struct
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CATALOG_JS = ROOT / "web" / "playground-src" / "catalog.js"
LOCK = ROOT / "web" / "datasets.lock.json"
PLAZA = ROOT / "docs" / "plaza"
MANIFEST = PLAZA / "plaza.json"
CURATED = PLAZA / "plaza.curated.json"
PLAYGROUND = "https://caviri.github.io/rete/docs/playground.html"

# Fields that come from the network probe, not from the sources.
PROBED = ("formatVersion", "size")
UNDERSCORE_OUT = ("_dropped", "_probe")

NODE_EVAL = r"""
const fs = require('fs');
global.window = {};
const src = fs.readFileSync(process.argv[1], 'utf8').replace(/\bconst RETE_WD_LABELS\b/, 'var RETE_WD_LABELS');
new Function('window', src)(global.window);
const C = global.window.RETE_PLAYGROUND_CATALOG;
const keep = ['remoteBase', 'remoteToken', 'companionBase', 'companionToken', 'pmtiles',
              'datasets', 'datasetMeta', 'datasetExtra', 'companions'];
const out = {};
for (const k of keep) out[k] = C[k];
process.stdout.write(JSON.stringify(out));
"""


def load_catalog() -> dict:
    p = subprocess.run(["node", "-e", NODE_EVAL, str(CATALOG_JS)], capture_output=True, check=False)
    if p.returncode != 0:
        sys.exit("node could not evaluate catalog.js:\n" + p.stderr.decode("utf8", "replace"))
    return json.loads(p.stdout.decode("utf8"))


# --- text helpers ----------------------------------------------------------

def plain(md: str) -> str:
    s = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", md or "")
    s = re.sub(r"[*_`]+", "", s)
    return re.sub(r"\s+", " ", s).strip()


def blurb_from(desc: str, limit: int = 220) -> str:
    s = plain(desc)
    if len(s) <= limit:
        return s
    # whole sentences while they fit
    out = ""
    for sent in re.split(r"(?<=[.!?])\s+(?=[A-Z0-9(])", s):
        if len(out) + len(sent) + 1 > limit:
            break
        out = (out + " " + sent).strip()
    if len(out) >= 60:
        return out
    cut = s[:limit].rsplit(" ", 1)[0].rstrip(",;:- ")
    return cut + "…"


def title_from(key: str, label: str) -> str:
    m = re.match(r"^.+?\.rete\s+—\s+(.*)$", label or "")
    if not m:
        return key
    rest = m.group(1).strip()
    rest = re.sub(r",\s*remote,?\s*lazy\)\s*$", ")", rest)
    rest = re.sub(r"\s*\(remote,?\s*lazy\)\s*$", "", rest)
    return f"{key} — {rest}"


# --- derivation ------------------------------------------------------------

def companions_from(c: dict, base: str) -> list[dict]:
    out = []
    if c.get("parquetDir"):
        out.append({"kind": "parquet", "label": "per-table Parquet",
                    "url": f"{base}/{c['parquetDir'].strip('/')}/",
                    "note": "directory of .parquet tables"})
    if c.get("parquet"):
        out.append({"kind": "parquet", "label": Path(c["parquet"]).name,
                    "url": f"{base}/{c['parquet']}",
                    **({"size": c["parquetSize"]} if c.get("parquetSize") else {})})
    if c.get("duckdb"):
        out.append({"kind": "duckdb", "label": Path(c["duckdb"]).name,
                    "url": f"{base}/{c['duckdb']}",
                    **({"size": c["duckdbSize"]} if c.get("duckdbSize") else {})})
    if c.get("sqlite"):
        out.append({"kind": "sqlite", "label": Path(c["sqlite"]).name,
                    "url": f"{base}/{c['sqlite']}",
                    **({"size": c["sqliteSize"]} if c.get("sqliteSize") else {})})
    return out


def derive(cat: dict, lock: dict, curated: dict) -> list[dict]:
    over = curated.get("overrides", {})
    comp_base = cat.get("companionBase") or cat["remoteBase"]
    entries = []
    for d in cat["datasets"]:
        key = d["key"]
        meta = cat.get("datasetMeta", {}).get(key, {})
        extra = cat.get("datasetExtra", {}).get(key, {})
        e: dict = {
            "key": key,
            "title": title_from(key, d.get("label", "")),
            "rete": d["url"],
            "kind": "remote-lazy",
        }
        if extra.get("icon"):
            e["icon"] = extra["icon"]
        if extra.get("tags"):
            e["tags"] = list(extra["tags"])
        b = blurb_from(d.get("description", ""))
        if b:
            e["blurb"] = b
        if meta.get("license"):
            e["license"] = meta["license"]
        if d.get("typePredicate"):
            e["typePredicate"] = d["typePredicate"]
        if d.get("shards"):
            e["shards"] = list(d["shards"])
        links = []
        src = (meta.get("source") or "").strip()
        if re.fullmatch(r"https?://\S+", src):
            links.append({"label": "Source", "url": src})
        e["links"] = links
        if key in cat.get("companions", {}):
            comps = companions_from(cat["companions"][key], comp_base)
            if comps:
                e["companions"] = comps
        for pk, pv in (cat.get("pmtiles") or {}).items():
            if pk == key and pv.get("url"):
                e["pmtiles"] = {"url": pv["url"], "label": pv.get("label"),
                                "size": pv.get("size"), "embedded": bool(pv.get("embedded"))}
        l = lock.get(key)
        if l and l.get("size"):
            e["size"] = l["size"]
            e["formatVersion"] = l.get("formatVersion")
        # curated wins, field by field
        for k, v in over.get(key, {}).items():
            e[k] = v
        if not any("playground.html" in (x.get("url") or "") for x in e["links"]):
            e["links"] = [{"label": "Open in playground",
                           "url": f"{PLAYGROUND}#dataset={key}&mode=sparql"}] + e["links"]
        entries.append(e)
    # curated entries first (their order), then catalog order
    rank = {k: i for i, k in enumerate(over)}
    entries.sort(key=lambda e: rank.get(e["key"], len(rank)))
    return entries


# --- probing ---------------------------------------------------------------

DEAD = {403, 404, 410}


def probe_url(url: str) -> dict:
    """Ranged GET of bytes 0-1023. Returns {status, version, size, magic}."""
    last = None
    for attempt in range(3):
        req = urllib.request.Request(url, headers={"Range": "bytes=0-1023", "User-Agent": "plaza-manifest/1"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                body = r.read(1024)
                cr = r.headers.get("Content-Range") or ""
                m = re.search(r"/(\d+)\s*$", cr)
                size = int(m.group(1)) if m else (int(r.headers["Content-Length"]) if r.status == 200 and r.headers.get("Content-Length") else None)
                magic = body[:4] == b"RETE"
                return {"status": r.status, "magic": magic, "version": body[4] if magic and len(body) > 4 else None,
                        "size": size}
        except urllib.error.HTTPError as ex:
            if ex.code in DEAD:
                return {"status": ex.code, "magic": False, "version": None, "size": None}
            last = f"HTTP {ex.code}"
        except Exception as ex:  # timeouts, DNS, resets
            last = repr(ex)
    return {"status": "error", "error": last}


def bundled_info(rel: str) -> dict:
    p = PLAZA / rel
    if not p.is_file():
        return {"status": 404, "magic": False, "version": None, "size": None}
    with open(p, "rb") as fh:
        head = fh.read(1024)
    magic = head[:4] == b"RETE"
    return {"status": "file", "magic": magic, "version": head[4] if magic else None, "size": p.stat().st_size}


def probe_all(entries: list[dict]) -> tuple[list[dict], list[dict], dict]:
    """Probe every URL; return (kept entries, dropped records, probe table)."""
    jobs: dict[str, str] = {}   # url -> role, deduplicated
    for e in entries:
        if e["kind"] == "remote-lazy":
            jobs[e["rete"]] = "rete"
        for s in e.get("shards", []):
            jobs.setdefault(s, "shard")
        if e.get("pmtiles") and not e["pmtiles"].get("embedded"):
            jobs.setdefault(e["pmtiles"]["url"], "pmtiles")
        for c in e.get("companions", []):
            if not c["url"].endswith("/"):
                jobs.setdefault(c["url"], "companion")
    results: dict[str, dict] = {}
    with cf.ThreadPoolExecutor(max_workers=8) as ex:
        futs = {ex.submit(probe_url, u): u for u in jobs}
        for f in cf.as_completed(futs):
            results[futs[f]] = f.result()
    flaky = {u: r for u, r in results.items() if r["status"] == "error"}
    if flaky:
        lines = "\n".join(f"  {u}: {r['error']}" for u, r in sorted(flaky.items()))
        sys.exit("probe failed on transient errors (nothing written):\n" + lines)

    kept, dropped = [], []
    for e in entries:
        if e["kind"] == "bundled":
            r = bundled_info(e["rete"])
        else:
            r = results[e["rete"]]
        ok = r["status"] in (200, 206, "file") and r["magic"]
        if not ok:
            dropped.append({"key": e["key"], "url": e["rete"],
                            "reason": f"HTTP {r['status']}" if r["status"] not in (200, 206, "file") else "not a .rete file"})
            continue
        e = dict(e)
        e["formatVersion"] = r["version"]
        if e.get("shards"):
            sizes = [results[s]["size"] for s in e["shards"]]
            if all(isinstance(x, int) for x in sizes):
                e["size"] = sum(sizes)
        elif r["size"]:
            e["size"] = r["size"]
        if e.get("companions"):
            good = []
            for c in e["companions"]:
                if c["url"].endswith("/"):
                    good.append(c)          # a directory: cannot be range-probed
                    continue
                cr = results[c["url"]]
                if cr["status"] in (200, 206):
                    good.append({**c, "verified": True})
                else:
                    dropped.append({"key": e["key"], "companion": c["url"], "reason": f"HTTP {cr['status']}"})
            if good:
                e["companions"] = good
            else:
                e.pop("companions")
        if e.get("pmtiles") and not e["pmtiles"].get("embedded"):
            if results[e["pmtiles"]["url"]]["status"] not in (200, 206):
                dropped.append({"key": e["key"], "pmtiles": e["pmtiles"]["url"],
                                "reason": f"HTTP {results[e['pmtiles']['url']]['status']}"})
                e.pop("pmtiles")
        kept.append(e)
    shard_bad = [{"key": e["key"], "shard": s, "reason": f"HTTP {results[s]['status']}"}
                 for e in kept for s in e.get("shards", []) if results[s]["status"] not in (200, 206)]
    return kept, dropped + shard_bad, results


# --- assembling / checking -------------------------------------------------

def assemble(cat, lock, curated, probe: bool, prior: dict | None) -> dict:
    entries = derive(cat, lock, curated)
    top = dict(curated.get("manifest", {}))
    top.setdefault("remoteBase", cat["remoteBase"])
    top.setdefault("remoteToken", cat.get("remoteToken", ""))
    if probe:
        entries, dropped, _ = probe_all(entries)
    else:
        # keep the previous probe results for entries we still list; honour the
        # previous drops so an offline run does not resurrect dead URLs
        prev = {e["key"]: e for e in (prior or {}).get("datasets", [])}
        dropped = [x for x in (prior or {}).get("_dropped", [])]
        gone = {x["key"] for x in dropped if "companion" not in x and "pmtiles" not in x and "shard" not in x}
        entries = [e for e in entries if e["key"] not in gone]
        for e in entries:
            p = prev.get(e["key"], {})
            for k in PROBED:
                if k in p and k not in curated.get("overrides", {}).get(e["key"], {}):
                    e[k] = p[k]
            if "companions" in e and "companions" in p and "companions" not in curated.get("overrides", {}).get(e["key"], {}):
                by_url = {c["url"]: c for c in p["companions"]}
                dead = {x["companion"] for x in dropped if x["key"] == e["key"] and "companion" in x}
                e["companions"] = [by_url.get(c["url"], c) for c in e["companions"] if c["url"] not in dead]
                if not e["companions"]:
                    e.pop("companions")
    out = {**top, "datasets": entries}
    if dropped:
        out["_dropped"] = dropped
    return out


def dump(obj: dict) -> str:
    return json.dumps(obj, indent=2, ensure_ascii=False) + "\n"


def strip_probed(m: dict) -> dict:
    m = json.loads(json.dumps(m))
    m.pop("_probe", None)
    for e in m.get("datasets", []):
        for k in PROBED:
            e.pop(k, None)
        for c in e.get("companions", []):
            c.pop("verified", None)
    return m


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="exit 1 if plaza.json is out of date; write nothing")
    ap.add_argument("--probe", action="store_true", help="with --check: also re-probe every URL")
    ap.add_argument("--no-probe", action="store_true", help="derive only, do not touch the network")
    a = ap.parse_args()

    cat = load_catalog()
    lock = {d["key"]: d for d in json.loads(LOCK.read_text("utf8"))["datasets"]} if LOCK.exists() else {}
    curated = json.loads(CURATED.read_text("utf8"))
    prior = json.loads(MANIFEST.read_text("utf8")) if MANIFEST.exists() else None

    if a.check:
        want_probe = a.probe
        want = assemble(cat, lock, curated, want_probe, prior)
        have = prior or {}
        if not want_probe:
            want, have = strip_probed(want), strip_probed(have)
        if dump(want) == dump(have):
            print(f"plaza.json is up to date ({len(want['datasets'])} datasets)")
            return 0
        wk = [e["key"] for e in want["datasets"]]
        hk = [e["key"] for e in have.get("datasets", [])]
        print("plaza.json is OUT OF DATE; run: python scripts/plaza_manifest.py", file=sys.stderr)
        print(f"  derived {len(wk)} datasets, file has {len(hk)}", file=sys.stderr)
        for k in sorted(set(wk) - set(hk)):
            print(f"  missing from plaza.json: {k}", file=sys.stderr)
        for k in sorted(set(hk) - set(wk)):
            print(f"  stale in plaza.json: {k}", file=sys.stderr)
        for e in want["datasets"]:
            h = next((x for x in have.get("datasets", []) if x["key"] == e["key"]), None)
            if h is not None and h != e:
                diff = [k for k in sorted(set(e) | set(h)) if e.get(k) != h.get(k)]
                print(f"  {e['key']}: differs in {', '.join(diff)}", file=sys.stderr)
        return 1

    out = assemble(cat, lock, curated, not a.no_probe, prior)
    MANIFEST.write_text(dump(out), "utf8", newline="\n")
    n = len(out["datasets"])
    print(f"wrote {MANIFEST.relative_to(ROOT)}: {n} datasets, {len(out.get('_dropped', []))} dropped/unreadable URLs")
    for x in out.get("_dropped", []):
        print("  dropped:", json.dumps(x))
    fmts: dict = {}
    for e in out["datasets"]:
        fmts[e.get("formatVersion")] = fmts.get(e.get("formatVersion"), 0) + 1
    print("  formats:", {("0x%02x" % k if k else None): v for k, v in sorted(fmts.items(), key=lambda kv: kv[0] or 0)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
