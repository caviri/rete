#!/usr/bin/env python3
"""Zenodo biosyslit (Biodiversity Literature Repository) Parquet -> N-Triples.

Reads data/zenodo/parquet-biosyslit/part-*.parquet (Zenodo-native JSON records
for the BLR community) and emits a richer graph than the bare metadata: the
Darwin Core taxonomy (dwc:kingdom … dwc:genus), community membership, files,
IIIF manifests and usage stats — while still honoring the scholar-alignment
canonical-IRI policy (work DOI -> https://doi.org/{doi}, ORCID creators ->
https://orcid.org/{orcid}) so BLR records join the rest of the scholarly graph.

Taxonomic treatments are typed zen:TaxonomicTreatment (⊑ dcite:Text); other
records get their DataCite resource class. Related identifiers (to journal
articles, Plazi, other treatments) become dcite: relation edges.

Usage:
  python biosyslit_to_nt.py                    # full -> data/zenodo/nt-biosyslit/
  python biosyslit_to_nt.py --limit-files 1    # quick slice
"""

import argparse
import os
import glob
from concurrent.futures import ProcessPoolExecutor

import orjson
import pyarrow.parquet as pq

HERE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DATA = os.path.join(HERE, "data", "zenodo")
IN_DIR = os.path.join(DATA, "parquet-biosyslit")
OUT_DIR = os.path.join(DATA, "nt-biosyslit")

DOI = "https://doi.org/"
ORCID = "https://orcid.org/"
ZREC = "https://zenodo.org/records/"
ZCOMM = "https://zenodo.org/communities/"
ZEN = "https://w3id.org/rete/zenodo#"
DCITE = "https://w3id.org/rete/datacite#"
DWC = "http://rs.tdwg.org/dwc/terms/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
RDFS_SEEALSO = "http://www.w3.org/2000/01/rdf-schema#seeAlso"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"

COMMUNITY_LABEL = {"biosyslit": "Biodiversity Literature Repository"}

# Zenodo resource_type id -> (extra rdf:type, dcite class)
RT_MAP = {
    "publication-taxonomictreatment": (ZEN + "TaxonomicTreatment", "Text"),
    "publication-article": (None, "JournalArticle"),
    "publication-preprint": (None, "Preprint"),
    "publication-conferencepaper": (None, "ConferencePaper"),
    "publication-book": (None, "Book"),
    "publication-section": (None, "Book"),
    "publication-report": (None, "Report"),
    "publication-thesis": (None, "Dissertation"),
    "publication": (None, "Text"),
    "dataset": (None, "Dataset"),
    "software": (None, "Software"),
    "image": (None, "Image"),
    "image-figure": (None, "Image"),
    "image-photo": (None, "Image"),
    "video": (None, "Audiovisual"),
}

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


def bare_doi(v):
    v = (v or "").strip().lower()
    for p in ("https://doi.org/", "http://doi.org/", "doi:"):
        if v.startswith(p):
            v = v[len(p):]
    return v


def rel_obj_iri(scheme, ident):
    s = (scheme or "").lower()
    v = (ident or "").strip()
    if not v:
        return None
    if s == "doi":
        return valid_iri(DOI + bare_doi(v))
    if s == "url":
        return valid_iri(v) if v.startswith(("http://", "https://")) else None
    if s == "arxiv":
        return valid_iri("https://arxiv.org/abs/" + v.replace("arXiv:", "").replace("arxiv:", ""))
    if s == "pmid":
        return valid_iri("https://pubmed.ncbi.nlm.nih.gov/" + v)
    if s == "handle":
        return valid_iri("https://hdl.handle.net/" + v)
    return None


def convert(args):
    in_path, out_path = args
    n_triples = 0
    seen_comm = set()
    fh = open(out_path, "w", encoding="utf-8", newline="\n")
    w = fh.write

    def t(s, p, o):
        return f"<{s}> <{p}> <{o}> .\n"

    def tl(s, p, val, dt=None):
        lit = '"' + esc(val) + '"'
        if dt:
            lit += "^^<" + dt + ">"
        return f"<{s}> <{p}> {lit} .\n"

    cols = ["doi", "record_id", "parent_doi", "created", "updated", "publication_date",
            "publisher", "resource_type_id", "resource_type_title", "title",
            "access_status", "communities", "views", "unique_views", "downloads",
            "unique_downloads", "description", "creators_json", "subjects_json",
            "identifiers_json", "related_identifiers_json", "rights_json",
            "custom_fields_json", "files_json", "iiif_manifest"]
    pf = pq.ParquetFile(in_path)
    for batch in pf.iter_batches(batch_size=20000, columns=cols):
        d = batch.to_pydict()
        rid_col = d["record_id"]
        for i in range(len(rid_col)):
            rid = rid_col[i]
            doi = d["doi"][i]
            buf = []
            subj = valid_iri(DOI + bare_doi(doi)) if doi else (valid_iri(ZREC + str(rid)) if rid else None)
            if not subj:
                continue

            buf.append(t(subj, RDF_TYPE, ZEN + "Record"))
            rid_type = d["resource_type_id"][i]
            extra, cls = RT_MAP.get(rid_type, (None, "OtherResource"))
            buf.append(t(subj, RDF_TYPE, DCITE + cls))
            if extra:
                buf.append(t(subj, RDF_TYPE, extra))

            if doi:
                buf.append(tl(subj, DCITE + "doi", bare_doi(doi)))
            if rid:
                buf.append(tl(subj, ZEN + "recordId", str(rid)))
                buf.append(t(subj, DCITE + "landingPage", ZREC + str(rid)))
            title = d["title"][i]
            if title:
                buf.append(tl(subj, DCITE + "title", title))
                buf.append(tl(subj, RDFS_LABEL, title))
            rtt = d["resource_type_title"][i]
            if rtt:
                buf.append(tl(subj, DCITE + "resourceType", rtt))
            pub = d["publisher"][i]
            if pub:
                buf.append(tl(subj, DCITE + "publisherName", pub))
            pd = d["publication_date"][i]
            if pd:
                buf.append(tl(subj, ZEN + "publicationDate", pd))
            created = d["created"][i]
            if created:
                buf.append(tl(subj, DCITE + "created", created))
            updated = d["updated"][i]
            if updated:
                buf.append(tl(subj, DCITE + "updated", updated))
            desc = d["description"][i]
            if desc:
                buf.append(tl(subj, DCT + "description", desc))
            acc = d["access_status"][i]
            if acc:
                buf.append(tl(subj, ZEN + "accessStatus", acc))
            iiif = valid_iri(d["iiif_manifest"][i])
            if iiif:
                buf.append(t(subj, ZEN + "iiifManifest", iiif))
            for col, prop in (("views", "views"), ("unique_views", "uniqueViews"),
                              ("downloads", "downloads"), ("unique_downloads", "uniqueDownloads")):
                v = d[col][i]
                if v is not None:
                    buf.append(tl(subj, ZEN + prop, str(v), dt=XSD + "integer"))

            # concept (parent) DOI -> version edge
            pdoi = d["parent_doi"][i]
            if pdoi:
                pb = bare_doi(pdoi)
                buf.append(tl(subj, ZEN + "conceptDoi", pb))
                po = valid_iri(DOI + pb)
                if po:
                    buf.append(t(subj, DCITE + "isVersionOf", po))

            # communities
            cj = d["communities"][i]
            if cj:
                try:
                    for slug in orjson.loads(cj):
                        ci = valid_iri(ZCOMM + str(slug))
                        if not ci:
                            continue
                        buf.append(t(subj, ZEN + "inCommunity", ci))
                        if slug not in seen_comm:
                            seen_comm.add(slug)
                            buf.append(t(ci, RDF_TYPE, ZEN + "Community"))
                            buf.append(tl(ci, ZEN + "communitySlug", str(slug)))
                            buf.append(tl(ci, RDFS_LABEL, COMMUNITY_LABEL.get(slug, str(slug))))
                except orjson.JSONDecodeError:
                    pass

            # creators
            cr = d["creators_json"][i]
            if cr:
                try:
                    for c in orjson.loads(cr):
                        po = c.get("person_or_org") or {}
                        name = po.get("name") or c.get("name")
                        orcid = None
                        for nid in (po.get("identifiers") or c.get("identifiers") or []):
                            if (nid.get("scheme") or "").lower() == "orcid":
                                orcid = (nid.get("identifier") or "").strip().rsplit("/", 1)[-1]
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
                except orjson.JSONDecodeError:
                    pass

            # subjects (keywords)
            sj = d["subjects_json"][i]
            if sj:
                try:
                    for s in orjson.loads(sj):
                        kw = s.get("subject") if isinstance(s, dict) else s
                        if kw:
                            buf.append(tl(subj, DCT + "subject", kw))
                except orjson.JSONDecodeError:
                    pass

            # own identifiers (Plazi treatment URL, etc.)
            ij = d["identifiers_json"][i]
            if ij:
                try:
                    for idd in orjson.loads(ij):
                        val = idd.get("identifier")
                        sc = (idd.get("scheme") or "").lower()
                        if not val:
                            continue
                        if sc == "url" and valid_iri(val):
                            buf.append(t(subj, RDFS_SEEALSO, val.strip()))
                        else:
                            buf.append(tl(subj, DCT + "identifier", val))
                except orjson.JSONDecodeError:
                    pass

            # rights / license (BLR shape: [{"id":"cc-by-4.0","props":{"url":...}}])
            rj = d["rights_json"][i]
            if rj:
                try:
                    for r in orjson.loads(rj):
                        if not isinstance(r, dict):
                            continue
                        link = valid_iri((r.get("props") or {}).get("url"))
                        if link:
                            buf.append(t(subj, DCT + "license", link))
                        elif r.get("id"):
                            buf.append(tl(subj, DCT + "license", str(r["id"])))
                except orjson.JSONDecodeError:
                    pass

            # Darwin Core taxonomy + journal
            cf = d["custom_fields_json"][i]
            if cf:
                try:
                    for k, v in orjson.loads(cf).items():
                        vals = v if isinstance(v, list) else [v]
                        for val in vals:
                            if val in (None, ""):
                                continue
                            if k.startswith("dwc:"):
                                buf.append(tl(subj, DWC + k[4:], str(val)))
                            elif k == "journal:journal":
                                jv = val.get("title") if isinstance(val, dict) else val
                                if jv:
                                    buf.append(tl(subj, ZEN + "journalTitle", str(jv)))
                            elif k == "openbiodiv:TaxonomicConceptLabel":
                                buf.append(tl(subj, DCT + "subject", str(val)))
                except (orjson.JSONDecodeError, AttributeError):
                    pass

            # files
            fj = d["files_json"][i]
            if fj and rid:
                try:
                    entries = orjson.loads(fj)
                    if isinstance(entries, dict):
                        for key, meta in entries.items():
                            firi = valid_iri(f"{ZREC}{rid}/files/{key}")
                            if not firi:
                                continue
                            buf.append(t(subj, ZEN + "hasFile", firi))
                            buf.append(t(firi, RDF_TYPE, ZEN + "File"))
                            buf.append(tl(firi, ZEN + "fileName", key))
                            if meta.get("mimetype"):
                                buf.append(tl(firi, ZEN + "mediaType", meta["mimetype"]))
                            if meta.get("size") is not None:
                                buf.append(tl(firi, ZEN + "byteSize", str(meta["size"]), dt=XSD + "integer"))
                            if meta.get("checksum"):
                                buf.append(tl(firi, ZEN + "checksum", meta["checksum"]))
                except orjson.JSONDecodeError:
                    pass

            # related identifiers -> relation network
            rij = d["related_identifiers_json"][i]
            if rij:
                try:
                    for r in orjson.loads(rij):
                        rt = r.get("relation_type") or {}
                        pred = REL_PRED.get((rt.get("id") if isinstance(rt, dict) else rt or "").lower())
                        if not pred:
                            continue
                        obj = rel_obj_iri(r.get("scheme"), r.get("identifier"))
                        if obj:
                            buf.append(t(subj, DCITE + pred, obj))
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
    ap.add_argument("--limit-files", type=int, default=None)
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
