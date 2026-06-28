#!/usr/bin/env python3
"""Harvest the Institucion Colombina's ARCAS catalogue (Albala 7) public records.

albala.icolombina.es serves the Archivo Capitular de Sevilla + Biblioteca Colombina
+ Archivo del Arzobispado through Baratz "Albala 7", a JS SPA over a proprietary
`data-handler/<pageid>/<action>/` API backed by Solr. The public consultation is
reachable ANONYMOUSLY (access level ACL2) via the results page:

    GET /albala/page?pageid=30000&responseIdentifier=searchResults
        &responseType=solr&start=<n>&rows=<r>&fq=*:*

which returns response.search_response.results[] (numFound = 69,900). Each record's
fields[] carry id, title, dates (fechas_ss), signatura (signatura_view_s), doc type,
archive, hierarchy (facet_parentRecordId / facet_recordId), reference code and the
coded ISAD elements (TI**_s). No login, no bulk dump, no OAI/IIIF.

This paginates the whole index at rows=1000 (~70 polite requests) and writes every
record (all fields) to records.ndjson.gz. Resumable via a .progress marker.
"""
import json, gzip, time, os, sys, urllib.request, urllib.parse, urllib.error

BASE = "https://albala.icolombina.es/albala"
UA = ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36")
ROWS = 1000
DELAY = 1.0          # polite delay between page requests (seconds)
OUTDIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                      "data", "albala")
NDJSON = os.path.join(OUTDIR, "records.ndjson.gz")
PROGRESS = os.path.join(OUTDIR, ".progress")

import http.cookiejar
cj = http.cookiejar.CookieJar()
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj))
opener.addheaders = [("User-Agent", UA), ("X-Requested-With", "XMLHttpRequest")]


def get(url, timeout=90):
    for attempt in range(5):
        try:
            with opener.open(url, timeout=timeout) as r:
                return r.read()
        except (urllib.error.URLError, TimeoutError) as e:
            wait = 3 * (attempt + 1)
            sys.stderr.write(f"  retry {attempt+1} ({e}) in {wait}s\n")
            time.sleep(wait)
    raise RuntimeError(f"failed: {url}")


def session():
    opener.open(BASE + "/", timeout=30).read()
    opener.open(BASE + "/data-handler/10600/saveUserSession/", timeout=30).read()


def rec_fields(r):
    out = {}
    for f in r.get("fields") or []:
        name = f.get("name")
        if name is not None:
            out[name] = f.get("values") or []
    return out


def fetch_page(start):
    q = urllib.parse.urlencode({
        "pageid": "30000", "responseIdentifier": "searchResults",
        "responseType": "solr", "start": str(start), "rows": str(ROWS), "fq": "*:*",
    })
    data = json.loads(get(f"{BASE}/page?{q}"))
    return data["response"]["search_response"]


def main():
    session()
    sr = fetch_page(0)
    total = int(sr.get("numFound") or 0)
    print(f"numFound = {total}; rows={ROWS} -> {(total + ROWS - 1)//ROWS} pages")

    done = set()
    if os.path.exists(PROGRESS):
        done = set(int(x) for x in open(PROGRESS).read().split() if x.strip())
    mode = "ab" if done else "wb"
    out = gzip.open(NDJSON, mode)

    n = 0
    for start in range(0, total, ROWS):
        if start in done:
            continue
        sr = fetch_page(start) if start != 0 else sr
        results = sr.get("results") or []
        for r in results:
            out.write((json.dumps(rec_fields(r), ensure_ascii=False) + "\n").encode("utf-8"))
            n += 1
        out.flush()
        with open(PROGRESS, "a") as p:
            p.write(f"{start}\n")
        print(f"  start={start:6d} +{len(results)} (total written {n})", flush=True)
        time.sleep(DELAY)
    out.close()
    print(f"DONE: {n} records -> {NDJSON}")


if __name__ == "__main__":
    main()
