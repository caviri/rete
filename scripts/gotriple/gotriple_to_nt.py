#!/usr/bin/env python3
"""GoTriple Parquet -> N-Triples for a rete scholarly graph.

Reads data/go-triple/parquet/*.parquet (one row per SSH publication) and emits
N-Triples using the gtr: vocabulary (data/go-triple/go-triple.ttl), honoring the
scholar-alignment CANONICAL-IRI policy so the graph auto-joins the rest of the
scholarly graph on DOI:

  * a document WITH a DOI  -> https://doi.org/{doi}            (lowercased, bare)
  * a document WITHOUT one -> https://w3id.org/rete/gotriple/document/{id}

Each document links to its GoTriple discipline (gtr:inDiscipline -> the SKOS
concept), its linked SSH-LCSH subject authorities (gtr:knowsAbout -> gtr:Subject
nodes minted at their semantics.gr URIs, so they dedupe across the corpus into a
real subject graph), authors (dcterms:creator literals), keywords, and its
provider / publisher / language / access / full-text URL.

The go-triple.ttl ontology is built into the .rete alongside these shards, so
the discipline labels and class definitions travel inside the file.

One worker per discipline parquet -> one NT shard; `rete build` merges them.

Usage:
  python gotriple_to_nt.py                 # full -> data/go-triple/nt/
  python gotriple_to_nt.py --limit 5000    # test slice
"""

import argparse
import glob
import os
import urllib.parse
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DATA = os.path.join(HERE, "data", "go-triple")
IN_DIR = os.path.join(DATA, "parquet")
OUT_DIR = os.path.join(DATA, "nt")

DOI = "https://doi.org/"
GTR = "https://w3id.org/rete/gotriple#"
GDOC = "https://w3id.org/rete/gotriple/document/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
SKOS_PREF = "http://www.w3.org/2004/02/skos/core#prefLabel"
DCT = "http://purl.org/dc/terms/"
_FORBIDDEN = set(' <>"{}|^`\\') | {chr(c) for c in range(0x21)}


def esc(s):
    return (str(s).replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", " ").replace("\r", " ").replace("\t", " "))


def valid_iri(s):
    if not s:
        return None
    s = s.strip()
    if not s or any(c in _FORBIDDEN for c in s):
        return None
    return s


def _label(js):
    """First English (else original/first) text from a CommonTranslatedLabel JSON array."""
    if not js:
        return None
    try:
        arr = orjson.loads(js)
    except orjson.JSONDecodeError:
        return None
    en = [x for x in arr if isinstance(x, dict) and x.get("lang") == "en" and x.get("text")]
    orig = [x for x in arr if isinstance(x, dict) and x.get("translated") in ("false", False) and x.get("text")]
    for x in (en + orig + [y for y in arr if isinstance(y, dict) and y.get("text")]):
        t = x.get("text")
        if t and t.strip():
            return t.strip()
    return None


def convert(args):
    in_path, out_path, limit = args
    n_triples = 0
    n_docs = 0
    seen_subj = set()
    fh = open(out_path, "w", encoding="utf-8", newline="\n")
    w = fh.write

    def t(s, p, o):
        return f"<{s}> <{p}> <{o}> .\n"

    def tl(s, p, v):
        return f'<{s}> <{p}> "{esc(v)}" .\n'

    cols = ["id", "doi", "title", "discipline", "date_published", "language",
            "provider", "publisher", "url", "conditions_of_access_json",
            "author_json", "keywords_json", "knows_about_json", "headline_json"]
    pf = pq.ParquetFile(in_path)
    for batch in pf.iter_batches(batch_size=20000, columns=cols):
        d = batch.to_pydict()
        n = len(d["id"])
        for i in range(n):
            if limit and n_docs >= limit:
                break
            rid = d["id"][i]
            doi = d["doi"][i]
            if doi:
                subj = valid_iri(DOI + doi.strip().lower())
            elif rid:
                subj = valid_iri(GDOC + urllib.parse.quote(str(rid), safe=""))
            else:
                continue
            if not subj:
                continue
            n_docs += 1
            buf = [t(subj, RDF_TYPE, GTR + "Document")]
            if doi:
                buf.append(tl(subj, GTR + "doi", doi.strip().lower()))
            if rid:
                buf.append(tl(subj, GTR + "gotripleId", str(rid)))
            title = d["title"][i] or _label(d["headline_json"][i])
            if title:
                buf.append(tl(subj, GTR + "title", title))
                buf.append(tl(subj, RDFS_LABEL, title))
            disc = d["discipline"][i]
            if disc:
                buf.append(t(subj, GTR + "inDiscipline", GTR + "discipline-" + disc))
            dp = d["date_published"][i]
            if dp:
                buf.append(tl(subj, GTR + "datePublished", dp))
            lang = d["language"][i]
            if lang:
                buf.append(tl(subj, GTR + "language", lang))
            prov = d["provider"][i]
            if prov:
                buf.append(tl(subj, GTR + "provider", prov))
            pub = d["publisher"][i]
            if pub:
                buf.append(tl(subj, GTR + "publisherName", pub))
            url = valid_iri(d["url"][i])
            if url and url.startswith(("http://", "https://")):
                buf.append(t(subj, GTR + "fullTextUrl", url))
            coa = d["conditions_of_access_json"][i]
            if coa:
                try:
                    for c in orjson.loads(coa):
                        if isinstance(c, str) and c and c != "undefined":
                            buf.append(tl(subj, GTR + "conditionsOfAccess", c))
                except orjson.JSONDecodeError:
                    pass
            # authors as literals (GoTriple author ids are not global; keep names)
            aj = d["author_json"][i]
            if aj:
                try:
                    for a in orjson.loads(aj):
                        name = a.get("fullname") if isinstance(a, dict) else None
                        if name and name.strip():
                            buf.append(tl(subj, DCT + "creator", name.strip()))
                except orjson.JSONDecodeError:
                    pass
            # keywords (English label)
            kj = d["keywords_json"][i]
            if kj:
                try:
                    for k in orjson.loads(kj):
                        txt = k.get("text") if isinstance(k, dict) else k
                        if txt and str(txt).strip() and str(txt) != "undefined":
                            buf.append(tl(subj, GTR + "keyword", str(txt).strip()))
                except orjson.JSONDecodeError:
                    pass
            # linked SSH-LCSH subjects -> shared nodes (dedupe across corpus by URI)
            kaj = d["knows_about_json"][i]
            if kaj:
                try:
                    for ka in orjson.loads(kaj):
                        uri = valid_iri(ka.get("uri")) if isinstance(ka, dict) else None
                        if not uri:
                            continue
                        buf.append(t(subj, GTR + "knowsAbout", uri))
                        if uri not in seen_subj:
                            seen_subj.add(uri)
                            buf.append(t(uri, RDF_TYPE, GTR + "Subject"))
                            labels = ka.get("labels") or []
                            en = next((l.get("text") for l in labels
                                       if isinstance(l, dict) and l.get("lang") == "en" and l.get("text")), None)
                            any_l = en or next((l.get("text") for l in labels
                                                if isinstance(l, dict) and l.get("text")), None)
                            if any_l:
                                buf.append(tl(uri, SKOS_PREF, any_l))
                                buf.append(tl(uri, RDFS_LABEL, any_l))
                except orjson.JSONDecodeError:
                    pass
            w("".join(buf))
            n_triples += len(buf)
    fh.close()
    return os.path.basename(out_path), n_docs, n_triples


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--in-dir", default=IN_DIR)
    ap.add_argument("--out-dir", default=OUT_DIR)
    ap.add_argument("--workers", type=int, default=min(14, max(2, os.cpu_count() - 4)))
    ap.add_argument("--limit", type=int, default=None, help="test: N docs per file")
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    files = sorted(glob.glob(os.path.join(args.in_dir, "*.parquet")))
    jobs = [(f, os.path.join(args.out_dir, os.path.splitext(os.path.basename(f))[0] + ".nt"), args.limit)
            for f in files]
    print(f"{len(jobs)} parquet files -> NT with {args.workers} workers", flush=True)

    tot_d = tot_t = 0
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for name, nd, nt in pool.map(convert, jobs):
            tot_d += nd
            tot_t += nt
            print(f"  {name:16s} {nd:>8,} docs  {nt:>10,} triples", flush=True)
    print(f"DONE: {tot_d:,} docs, {tot_t:,} triples across {len(jobs)} shards in {args.out_dir}", flush=True)


if __name__ == "__main__":
    main()
