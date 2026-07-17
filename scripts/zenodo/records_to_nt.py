#!/usr/bin/env python3
"""Zenodo metadata Parquet -> N-Triples for a rete scholarly graph.

Reads data/zenodo/parquet-metadata/part-*.parquet (one row per Zenodo record,
DataCite kernel-4.5 metadata) and emits N-Triples using the rete Zenodo/DataCite
vocabularies, honoring the scholar-alignment CANONICAL-IRI policy so the graph
auto-joins DataCite/OpenAIRE/ORCID/OpenCitations in a union:

  * a work with a DOI  -> https://doi.org/{doi}      (lowercased, bare)
  * a person with ORCID -> https://orcid.org/{orcid}
  * related DOIs        -> https://doi.org/{doi}      (so citation/version edges
                          land on the SAME node as the cited record)

Each record is typed both as zen:Record and as its DataCite resource class
(dcite:Dataset / dcite:Software / dcite:JournalArticle / …) so the schema
pyramid has meaningful communities. The relation network in
related_identifiers_json becomes dcite: edges (isVersionOf, isPartOf, cites, …)
— the connected graph. ORCID creators become shared person nodes.

Runs one worker per parquet file, each writing an NT shard; `rete build` merges
them.

Usage:
  python records_to_nt.py                    # full, -> data/zenodo/nt-metadata/
  python records_to_nt.py --limit-files 1    # quick slice
"""

import argparse
import os
import glob
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DATA = os.path.join(HERE, "data", "zenodo")
IN_DIR = os.path.join(DATA, "parquet-metadata")
OUT_DIR = os.path.join(DATA, "nt-metadata")

DOI = "https://doi.org/"
ORCID = "https://orcid.org/"
ZREC = "https://zenodo.org/records/"
ZEN = "https://w3id.org/rete/zenodo#"
DCITE = "https://w3id.org/rete/datacite#"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"

# DataCite resourceTypeGeneral -> dcite class local name (else OtherResource)
RT_CLASS = {
    "Dataset": "Dataset", "Software": "Software", "Text": "Text", "Image": "Image",
    "Audiovisual": "Audiovisual", "Sound": "Sound", "Collection": "Collection",
    "PhysicalObject": "PhysicalObject", "Preprint": "Preprint",
    "JournalArticle": "JournalArticle", "Book": "Book", "BookChapter": "Book",
    "ConferencePaper": "ConferencePaper", "Dissertation": "Dissertation",
    "Report": "Report", "Workflow": "Workflow", "Model": "Model",
    "ComputationalNotebook": "Software", "DataPaper": "Text",
}

# DataCite relationType (lowercased) -> dcite predicate local name
REL_PRED = {
    "isversionof": "isVersionOf", "hasversion": "hasVersion",
    "isnewversionof": "isNewVersionOf", "ispartof": "isPartOf", "haspart": "hasPart",
    "cites": "cites", "references": "references", "iscitedby": "isCitedBy",
    "isreferencedby": "isReferencedBy", "issupplementto": "isSupplementTo",
    "issupplementedby": "isSupplementedBy", "isderivedfrom": "isDerivedFrom",
    "issourceof": "isSourceOf", "isidenticalto": "isIdenticalTo",
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


def rel_obj_iri(idtype, ident):
    t = (idtype or "").upper()
    v = (ident or "").strip()
    if not v:
        return None
    if t == "DOI":
        v = v.lower()
        for p in ("https://doi.org/", "http://doi.org/", "doi:"):
            if v.startswith(p):
                v = v[len(p):]
        return valid_iri(DOI + v)
    if t == "URL":
        return valid_iri(v) if v.startswith(("http://", "https://")) else None
    if t == "ARXIV":
        return valid_iri("https://arxiv.org/abs/" + v.replace("arXiv:", "").replace("arxiv:", ""))
    if t == "PMID":
        return valid_iri("https://pubmed.ncbi.nlm.nih.gov/" + v)
    if t == "HANDLE":
        return valid_iri("https://hdl.handle.net/" + v)
    return None


def convert(args):
    in_path, out_path = args
    n_triples = 0
    fh = open(out_path, "w", encoding="utf-8", newline="\n")
    w = fh.write

    def t(s, p, o_iri):
        return f"<{s}> <{p}> <{o_iri}> .\n"

    def tl(s, p, val, dt=None, lang=None):
        lit = '"' + esc(val) + '"'
        if lang:
            lit += "@" + lang
        elif dt:
            lit += "^^<" + dt + ">"
        return f"<{s}> <{p}> {lit} .\n"

    cols = ["doi", "record_id", "publisher", "publication_year", "published",
            "updated", "resource_type_general", "resource_type", "title", "language",
            "version", "schema_version", "url", "creators_json", "subjects_json",
            "related_identifiers_json", "rights_list_json"]
    pf = pq.ParquetFile(in_path)
    for batch in pf.iter_batches(batch_size=20000, columns=cols):
        d = batch.to_pydict()
        rid_col = d["record_id"]
        for i in range(len(rid_col)):
            rid = rid_col[i]
            doi = d["doi"][i]
            buf = []
            if doi:
                subj = valid_iri(DOI + doi.strip().lower())
            else:
                subj = valid_iri(ZREC + str(rid)) if rid else None
            if not subj:
                continue

            # types: zen:Record + dcite resource class
            buf.append(t(subj, RDF_TYPE, ZEN + "Record"))
            rtg = d["resource_type_general"][i]
            cls = RT_CLASS.get(rtg, "OtherResource")
            buf.append(t(subj, RDF_TYPE, DCITE + cls))

            if doi:
                buf.append(tl(subj, DCITE + "doi", doi.strip().lower()))
            if rid:
                buf.append(tl(subj, ZEN + "recordId", str(rid)))
            title = d["title"][i]
            if title:
                buf.append(tl(subj, DCITE + "title", title))
                buf.append(tl(subj, RDFS_LABEL, title))
            pub = d["publisher"][i]
            if pub:
                buf.append(tl(subj, DCITE + "publisherName", pub))
            year = d["publication_year"][i]
            if year:
                buf.append(tl(subj, DCITE + "publicationYear", str(year), dt=XSD + "gYear"))
            published = d["published"][i]
            if published:
                buf.append(tl(subj, ZEN + "publicationDate", published))
            updated = d["updated"][i]
            if updated:
                buf.append(tl(subj, DCITE + "updated", updated))
            rt = d["resource_type"][i]
            if rt:
                buf.append(tl(subj, DCITE + "resourceType", rt))
            lang = d["language"][i]
            if lang:
                buf.append(tl(subj, DCITE + "language", lang))
            ver = d["version"][i]
            if ver:
                buf.append(tl(subj, DCITE + "version", ver))
            sv = d["schema_version"][i]
            if sv:
                buf.append(tl(subj, DCITE + "schemaVersion", sv))
            url = valid_iri(d["url"][i])
            if url:
                buf.append(t(subj, DCITE + "landingPage", url))

            # creators
            cj = d["creators_json"][i]
            if cj:
                try:
                    for c in orjson.loads(cj):
                        name = c.get("name")
                        orcid = None
                        for nid in (c.get("nameIdentifiers") or []):
                            if (nid.get("nameIdentifierScheme") or "").upper() == "ORCID":
                                orcid = (nid.get("nameIdentifier") or "").strip()
                                orcid = orcid.rsplit("/", 1)[-1]  # bare id
                                break
                        if name:
                            buf.append(tl(subj, DCT + "creator", name))
                        if orcid:
                            piri = valid_iri(ORCID + orcid)
                            if piri:
                                buf.append(t(subj, DCITE + "creator", piri))
                                buf.append(t(piri, RDF_TYPE, DCITE + "Agent"))
                                if name:
                                    buf.append(tl(piri, DCITE + "agentName", name))
                                    buf.append(tl(piri, RDFS_LABEL, name))
                                buf.append(tl(piri, DCITE + "orcid", orcid))
                                affs = c.get("affiliation") or []
                                if affs and isinstance(affs, list):
                                    buf.append(tl(piri, DCITE + "affiliationName", str(affs[0])))
                except orjson.JSONDecodeError:
                    pass

            # subjects
            sj = d["subjects_json"][i]
            if sj:
                try:
                    for s in orjson.loads(sj):
                        kw = s.get("subject") if isinstance(s, dict) else s
                        if kw:
                            buf.append(tl(subj, DCT + "subject", kw))
                except orjson.JSONDecodeError:
                    pass

            # rights / license
            rl = d["rights_list_json"][i]
            if rl:
                try:
                    for r in orjson.loads(rl):
                        uri = valid_iri(r.get("rightsUri"))
                        if uri:
                            buf.append(t(subj, DCT + "license", uri))
                except orjson.JSONDecodeError:
                    pass

            # related identifiers -> the relation network
            rij = d["related_identifiers_json"][i]
            if rij:
                try:
                    for r in orjson.loads(rij):
                        pred = REL_PRED.get((r.get("relationType") or "").lower())
                        if not pred:
                            continue
                        obj = rel_obj_iri(r.get("relatedIdentifierType"), r.get("relatedIdentifier"))
                        if obj:
                            buf.append(t(subj, DCITE + pred, obj))
                            if pred == "isVersionOf" and obj.startswith(DOI):
                                buf.append(tl(subj, ZEN + "conceptDoi", obj[len(DOI):]))
                except orjson.JSONDecodeError:
                    pass

            w("".join(buf))
            n_triples += len(buf)
    fh.close()
    return out_path, n_triples


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--in-dir", default=IN_DIR)
    ap.add_argument("--out-dir", default=OUT_DIR)
    ap.add_argument("--workers", type=int, default=min(16, max(2, os.cpu_count() - 4)))
    ap.add_argument("--limit-files", type=int, default=None, help="test: only N parquet files")
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    files = sorted(glob.glob(os.path.join(args.in_dir, "part-*.parquet")))
    if args.limit_files:
        files = files[: args.limit_files]
    jobs = [(f, os.path.join(args.out_dir, os.path.splitext(os.path.basename(f))[0] + ".nt"))
            for f in files]
    print(f"{len(jobs)} parquet files -> NT with {args.workers} workers", flush=True)

    total = 0
    with ProcessPoolExecutor(max_workers=args.workers) as pool:
        for out_path, n in pool.map(convert, jobs):
            total += n
            print(f"  {os.path.basename(out_path)}: {n:,} triples", flush=True)
    print(f"DONE: {total:,} triples across {len(jobs)} shards in {args.out_dir}", flush=True)


if __name__ == "__main__":
    main()
