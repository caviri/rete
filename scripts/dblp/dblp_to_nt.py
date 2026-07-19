#!/usr/bin/env python3
"""DBLP Parquet -> N-Triples for a rete scholarly graph.

Reads data/dblp/parquet/record/*.parquet (one row per DBLP publication) and
emits N-Triples using the dblp: vocabulary (data/dblp/dblp.ttl), honoring the
scholar-alignment CANONICAL-IRI policy so the graph auto-joins the rest of the
scholarly graph:

  * a publication WITH a DOI  -> https://doi.org/{doi}          (lowercased, bare)
  * a publication WITHOUT one -> https://dblp.org/rec/{key}     (the real DBLP page)
  * an author WITH an ORCID   -> https://orcid.org/{orcid}
  * an author WITHOUT one     -> https://w3id.org/rete/dblp/author/{name}
                                 (DBLP disambiguates by name suffix, e.g. "… 0001")

The co-authorship backbone comes from each record's authors_json ({name, orcid}),
so every author edge attaches to the correct record IRI without a key->IRI map.
Publications are typed dblp:Publication + a subtype (Article / Inproceedings /
Incollection / …) for a meaningful schema pyramid. The dblp.ttl ontology is
built into the .rete alongside these shards.

One worker per record parquet -> one NT shard; `rete build` merges them.

Usage:
  python dblp_to_nt.py                 # full -> data/dblp/nt/
  python dblp_to_nt.py --limit 5000    # test slice
"""

import argparse
import glob
import os
import urllib.parse
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DATA = os.path.join(HERE, "data", "dblp")
IN_DIR = os.path.join(DATA, "parquet", "record")
OUT_DIR = os.path.join(DATA, "nt")

DOI = "https://doi.org/"
ORCID = "https://orcid.org/"
DBLP_REC = "https://dblp.org/rec/"
DAUTHOR = "https://w3id.org/rete/dblp/author/"
DBLP = "https://w3id.org/rete/dblp#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDFS_SEEALSO = "http://www.w3.org/2000/01/rdf-schema#seeAlso"
XSD = "http://www.w3.org/2001/XMLSchema#"

# DBLP bibtex type -> dblp: subclass local name (None = only dblp:Publication)
TYPE_CLASS = {
    "article": "Article", "inproceedings": "Inproceedings",
    "incollection": "Incollection", "proceedings": "Proceedings",
    "book": "Book", "phdthesis": "Thesis", "mastersthesis": "Thesis",
}
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


def author_iri(name, orcid):
    if orcid:
        o = orcid.strip().rsplit("/", 1)[-1]
        iri = valid_iri(ORCID + o)
        if iri:
            return iri, o
    if name and name.strip():
        return valid_iri(DAUTHOR + urllib.parse.quote(name.strip(), safe="")), None
    return None, None


def convert(args):
    in_path, out_path, limit = args
    n_rec = 0
    n_triples = 0
    seen_author = set()
    fh = open(out_path, "w", encoding="utf-8", newline="\n")
    w = fh.write

    def t(s, p, o):
        return f"<{s}> <{p}> <{o}> .\n"

    def tl(s, p, v, dt=None):
        lit = '"' + esc(v) + '"'
        if dt:
            lit += "^^<" + dt + ">"
        return f"<{s}> <{p}> {lit} .\n"

    def emit_creators(buf, subj, js, pred):
        try:
            arr = orjson.loads(js)
        except (orjson.JSONDecodeError, TypeError):
            return
        for a in arr:
            if not isinstance(a, dict):
                continue
            name = a.get("name")
            iri, orcid = author_iri(name, a.get("orcid"))
            if not iri:
                continue
            buf.append(t(subj, DBLP + pred, iri))
            if iri not in seen_author:
                seen_author.add(iri)
                buf.append(t(iri, RDF_TYPE, DBLP + "Person"))
                if name:
                    buf.append(tl(iri, DBLP + "creatorName", name))
                    buf.append(tl(iri, RDFS_LABEL, name))
                if orcid:
                    buf.append(tl(iri, DBLP + "orcid", orcid))

    cols = ["key", "type", "title", "year", "venue", "volume", "number", "pages",
            "publisher", "isbn", "series", "doi", "authors_json", "editors_json", "ee_json"]
    pf = pq.ParquetFile(in_path)
    for batch in pf.iter_batches(batch_size=20000, columns=cols):
        d = batch.to_pydict()
        for i in range(len(d["key"])):
            if limit and n_rec >= limit:
                break
            key = d["key"][i]
            doi = d["doi"][i]
            btype = (d["type"][i] or "").strip()
            if doi:
                subj = valid_iri(DOI + doi.strip().lower())
            elif key:
                subj = valid_iri(DBLP_REC + urllib.parse.quote(key, safe="/"))
            else:
                continue
            if not subj:
                continue
            n_rec += 1
            buf = []
            if btype == "www":
                buf.append(t(subj, RDF_TYPE, DBLP + "PersonPage"))
            else:
                buf.append(t(subj, RDF_TYPE, DBLP + "Publication"))
                cls = TYPE_CLASS.get(btype)
                if cls:
                    buf.append(t(subj, RDF_TYPE, DBLP + cls))
            if btype:
                buf.append(tl(subj, DBLP + "bibtexType", btype))
            if key:
                buf.append(tl(subj, DBLP + "recordKey", key))
                buf.append(t(subj, DBLP + "url", valid_iri(DBLP_REC + urllib.parse.quote(key, safe="/")) or DBLP_REC))
            if doi:
                buf.append(tl(subj, DBLP + "doi", doi.strip().lower()))
            title = d["title"][i]
            if title:
                buf.append(tl(subj, DBLP + "title", title))
                buf.append(tl(subj, RDFS_LABEL, title))
            year = d["year"][i]
            if year and str(year).strip().isdigit():
                buf.append(tl(subj, DBLP + "year", str(year).strip(), dt=XSD + "gYear"))
            for col, prop in (("venue", "venue"), ("volume", "volume"), ("number", "number"),
                              ("pages", "pages"), ("publisher", "publisher"),
                              ("isbn", "isbn"), ("series", "series")):
                v = d[col][i]
                if v and str(v).strip():
                    buf.append(tl(subj, DBLP + prop, str(v).strip()))
            # electronic-edition links (skip the DOI url, already captured)
            ee = d["ee_json"][i]
            if ee:
                try:
                    for u in orjson.loads(ee):
                        iu = valid_iri(u)
                        if iu and iu.startswith(("http://", "https://")) and "doi.org/" not in iu:
                            buf.append(t(subj, RDFS_SEEALSO, iu))
                except orjson.JSONDecodeError:
                    pass
            if d["authors_json"][i]:
                emit_creators(buf, subj, d["authors_json"][i], "authoredBy")
            if d["editors_json"][i]:
                emit_creators(buf, subj, d["editors_json"][i], "editedBy")
            w("".join(buf))
            n_triples += len(buf)
    fh.close()
    return os.path.basename(out_path), n_rec, n_triples


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--in-dir", default=IN_DIR)
    ap.add_argument("--out-dir", default=OUT_DIR)
    ap.add_argument("--workers", type=int, default=min(8, max(2, os.cpu_count() - 4)))
    ap.add_argument("--limit", type=int, default=None, help="test: N records per file")
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    files = sorted(glob.glob(os.path.join(args.in_dir, "*.parquet")))
    jobs = [(f, os.path.join(args.out_dir, os.path.splitext(os.path.basename(f))[0] + ".nt"), args.limit)
            for f in files]
    print(f"{len(jobs)} record parquet files -> NT with {args.workers} workers", flush=True)

    tot_r = tot_t = 0
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for name, nr, nt in pool.map(convert, jobs):
            tot_r += nr
            tot_t += nt
            print(f"  {name:16s} {nr:>9,} records  {nt:>11,} triples", flush=True)
    print(f"DONE: {tot_r:,} records, {tot_t:,} triples across {len(jobs)} shards in {args.out_dir}", flush=True)


if __name__ == "__main__":
    main()
