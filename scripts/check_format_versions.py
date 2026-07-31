"""Probe the format-generation byte of every remote .rete the playground catalog
lists — one 8-byte Range request each.

Header layout: b"RETE" magic, then byte 4 = format version. The engine reads
0x05 only (MIN_STABLE_READ_VERSION..=CURRENT_FORMAT_VERSION); a pre-1.0 0x01-0x04
file errors with "unsupported .rete format" in every client, so a catalog entry
pointing at one is dead on arrival in the playground.

Usage: python scripts/check_format_versions.py [--json]
"""
import concurrent.futures as cf
import json
import re
import sys
import urllib.request

CAT = "web/playground-src/catalog.js"
UA = "rete-format-audit"


def urls():
    s = open(CAT, encoding="utf-8").read()
    out = {}
    # remote-lazy entries: {"key": "...", ..., "url": "https://..."}
    for m in re.finditer(r'\{"key":\s*"([^"]+)"[^}]*?"url":\s*"([^"]+\.rete)"', s):
        out.setdefault(m.group(1), m.group(2))
    # sharded datasets: "shards": ["https://…", …]
    for m in re.finditer(r'\{"key":\s*"([^"]+)"[^}]*?"shards":\s*\[([^\]]+)\]', s):
        for u in re.findall(r'"(https://[^"]+\.rete)"', m.group(2)):
            out.setdefault(m.group(1) + " [shard " + u.rsplit("/", 1)[-1] + "]", u)
    return out


def probe(item):
    key, url = item
    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Range": "bytes=0-7"})
        with urllib.request.urlopen(req, timeout=30) as r:
            b = r.read(8)
            code = r.status
        if b[:4] != b"RETE":
            return key, url, code, None, "NOT a .rete (bad magic)"
        ver = b[4]
        return key, url, code, ver, ("OK" if ver == 5 else "UNREADABLE — engine needs 0x05")
    except Exception as e:
        return key, url, None, None, f"FETCH FAILED: {str(e)[:60]}"


def main():
    items = sorted(urls().items())
    print(f"probing {len(items)} catalog .rete URLs (8 bytes each)\n", flush=True)
    rows = []
    with cf.ThreadPoolExecutor(max_workers=12) as ex:
        for key, url, code, ver, note in ex.map(probe, items):
            rows.append({"key": key, "url": url, "http": code, "version": ver, "note": note})
            if note != "OK":
                print(f"  !! {key:34s} v={ver} {note}", flush=True)
    bad = [r for r in rows if r["note"] != "OK"]
    print(f"\n{len(rows) - len(bad)}/{len(rows)} serve format 0x05")
    if bad:
        print("PROBLEMS:")
        for r in bad:
            print(f"  {r['key']:34s} v={r['version']}  {r['note']}\n      {r['url']}")
    if "--json" in sys.argv:
        json.dump(rows, open("data/_format_audit.json", "w"), indent=1)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
