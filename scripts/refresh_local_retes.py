"""Refresh stale local web/*.rete files to format generation 1 (0x05).

Files built before the format froze (0x01-0x04) are unreadable by every current
engine, and they are dangerous to leave lying around: a page builder that embeds
one ships a broken page (that is exactly how docs/explorer.html came to carry an
unreadable dataset). For each stale file we prefer the PUBLISHED copy — the
catalog URL is the release contract — and only report the ones with no published
counterpart, which need a rebuild from source or deletion.

Usage:
  python scripts/refresh_local_retes.py            # report only
  python scripts/refresh_local_retes.py --apply    # download the published 0x05
"""
import glob
import os
import re
import sys
import urllib.request

UA = "rete-local-refresh"
CAT = "web/playground-src/catalog.js"


def catalog_urls():
    s = open(CAT, encoding="utf-8").read()
    out = {}
    for m in re.finditer(r'\{"key":\s*"([^"]+)"[^}]*?"url":\s*"([^"]+\.rete)"', s):
        out.setdefault(m.group(1), m.group(2))
    return out


def version_of(path):
    with open(path, "rb") as f:
        head = f.read(8)
    return (head[4] if head[:4] == b"RETE" else None)


def remote_version(url):
    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Range": "bytes=0-7"})
        with urllib.request.urlopen(req, timeout=30) as r:
            b = r.read(8)
        return b[4] if b[:4] == b"RETE" else None
    except Exception:
        return None


def main():
    apply = "--apply" in sys.argv
    urls = catalog_urls()
    stale, fixed, orphan = [], [], []
    for p in sorted(glob.glob("web/*.rete")):
        v = version_of(p)
        if v is None or v == 5:
            continue
        key = os.path.basename(p)[:-5]
        stale.append((p, v, key))

    print(f"stale local .rete files (format < 0x05): {len(stale)}\n")
    for p, v, key in stale:
        url = urls.get(key)
        rv = remote_version(url) if url else None
        if url and rv == 5:
            if apply:
                tmp = p + ".new"
                req = urllib.request.Request(url, headers={"User-Agent": UA})
                with urllib.request.urlopen(req, timeout=600) as r, open(tmp, "wb") as f:
                    while True:
                        chunk = r.read(1 << 20)
                        if not chunk:
                            break
                        f.write(chunk)
                if version_of(tmp) == 5:
                    os.replace(tmp, p)
                    print(f"  FIXED  0x{v:02x} -> 0x05  {p}  ({os.path.getsize(p)/1e6:.1f} MB from R2)")
                    fixed.append(p)
                else:
                    os.remove(tmp)
                    print(f"  FAILED download did not verify: {p}")
            else:
                print(f"  would fix  0x{v:02x} -> published 0x05   {p}  <- {url}")
                fixed.append(p)
        else:
            print(f"  NO PUBLISHED COPY  0x{v:02x}  {p}"
                  + (f"  (catalog url {url} serves 0x{rv:02x})" if url and rv else "  (not in catalog)"))
            orphan.append(p)

    print(f"\n{len(fixed)} refreshable from R2, {len(orphan)} have no published 0x05:")
    for p in orphan:
        print(f"  {p}  — rebuild from source or delete (nothing reads it once it is stale)")
    if not apply and fixed:
        print("\nre-run with --apply to download them")


if __name__ == "__main__":
    main()
