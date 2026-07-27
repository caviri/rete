"""Stream the DataCite Parquet (metadata + PID-Links) -> N-Triples on stdout,
following the rete DataCite ontology (dcite: = https://w3id.org/rete/datacite#).
FULL model — every field kept; pipe into `rete build - --format nt --memory-budget-mb`.

Two table families (auto-detected by columns, or forced with --mode):
  metadata (parquet-2023/2024/2025): one DOI research output per row, subject IRI
    https://doi.org/<doi>. Emits all scalar fields + reified creators (dcite:creator
    shortcut + dcite:hasContributor->AgentRole->isHeldBy Agent, agents keyed to
    orcid.org where an ORCID nameIdentifier is present), related_identifiers as typed
    relation edges (Cites/IsSupplementTo/IsVersionOf/…), funding_references (dcite:fundedBy
    -> Funder), and subjects (dcterms:subject).
  links   (parquet-links-*): the PID Graph. Each row is a typed edge subj->obj PLUS a
    reified dcite:PidRelation carrying the asserting source and timestamp (no provenance
    dropped).

Usage: python parquet_to_nt.py                # all tables
       python parquet_to_nt.py --mode links --row-groups 1 > sample.nt
"""
import argparse
import glob
import json
import os
import re
import sys

import pyarrow.parquet as pq

D = "https://w3id.org/rete/datacite#"
DOIB = "https://doi.org/"
ORCIDB = "https://orcid.org/"
RORB = "https://ror.org/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
DCT_SUBJECT = "http://purl.org/dc/terms/subject"
XSD = "http://www.w3.org/2001/XMLSchema#"
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")

TYPE_CLASS = {
    "dataset": "Dataset", "software": "Software", "text": "Text", "image": "Image",
    "audiovisual": "Audiovisual", "sound": "Sound", "collection": "Collection",
    "physicalobject": "PhysicalObject", "preprint": "Preprint",
    "journalarticle": "JournalArticle", "book": "Book", "conferencepaper": "ConferencePaper",
    "dissertation": "Dissertation", "report": "Report", "workflow": "Workflow",
    "model": "Model", "other": "OtherResource",
}
# relatedIdentifier relationType (CamelCase) + PID-links relation_type (kebab) -> dcite property
REL_PROP = {
    "cites": "cites", "iscitedby": "isCitedBy", "references": "references",
    "isreferencedby": "isReferencedBy", "issupplementto": "isSupplementTo",
    "issupplementedby": "isSupplementedBy", "isversionof": "isVersionOf",
    "hasversion": "hasVersion", "isnewversionof": "isNewVersionOf",
    "ispartof": "isPartOf", "haspart": "hasPart", "isderivedfrom": "isDerivedFrom",
    "issourceof": "isSourceOf", "isidenticalto": "isIdenticalTo",
    "isrelatedto": "isRelatedTo", "isvariantformof": "isRelatedTo",
    "isauthoredby": "isAuthoredBy",
}
REL_CONCEPT = {  # for the reified PidRelation's dcite:relationType skos concept (subset with concepts)
    "cites": "rel-cites", "issupplementto": "rel-isSupplementTo",
    "isversionof": "rel-isVersionOf", "ispartof": "rel-isPartOf",
    "isderivedfrom": "rel-isDerivedFrom", "isidenticalto": "rel-isIdenticalTo",
    "references": "rel-references", "isauthoredby": "rel-isAuthoredBy",
}

_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')
_LIT = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_LIT_RE = re.compile(r'[\\"\n\r\t]')
_YEAR = re.compile(r'^\d{4}$')
_ORCID = re.compile(r'(\d{4}-\d{4}-\d{4}-\d{3}[\dxX])')


def ienc(s):
    return _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), s)


def lit(s):
    s = _LIT_RE.sub(lambda m: _LIT[m.group()], str(s))
    if any(ord(c) < 0x20 for c in s):
        s = "".join(c for c in s if ord(c) >= 0x20)
    return s


def relkey(rt):
    return re.sub(r'[^a-z]', '', (rt or "").lower())


def pid_iri(pid, ptype=None):
    """Map a PID string (DOI / ORCID / URL) to an IRI."""
    if not pid:
        return None
    p = pid.strip()
    low = p.lower()
    if low.startswith("http://") or low.startswith("https://"):
        if "orcid.org/" in low:
            m = _ORCID.search(low)
            return ORCIDB + m.group(1) if m else ienc(p)
        return ienc(p)
    if low.startswith("10.") and "/" in low:
        return DOIB + ienc(low)
    m = _ORCID.fullmatch(p)
    if m:
        return ORCIDB + m.group(1)
    if low.startswith("doi:"):
        return DOIB + ienc(low[4:])
    return DOIB + ienc(low) if low.startswith("10.") else ienc(p)


def jload(s):
    if not s:
        return None
    try:
        return json.loads(s)
    except Exception:
        return None


def emit_metadata(buf, C, i):
    doi = C["doi"][i]
    if not doi:
        return
    subj = DOIB + ienc(doi.strip().lower())

    def t(p, o):
        buf.append(f"<{subj}> <{p}> {o} .\n")

    rtg = (C["resource_type_general"][i] or "").replace(" ", "").lower()
    t(RDF_TYPE, f"<{D}{TYPE_CLASS.get(rtg, 'Resource')}>")
    t(f"{D}doi", f'"{lit(doi.strip().lower())}"')
    for col, prop, dt in [
        ("prefix", "prefix", None), ("title", "title", None),
        ("resource_type", "resourceType", None), ("publisher", "publisherName", None),
        ("version", "version", None), ("schema_version", "schemaVersion", None),
        ("state", "state", None), ("client_id", "clientId", None),
        ("language", "language", None), ("url", "landingPage", "anyURI"),
        ("registered", "registered", "dateTime"), ("created", "created", "dateTime"),
        ("updated", "updated", "dateTime"),
    ]:
        v = C.get(col, [None] * len(C["doi"]))[i]
        if v:
            if dt:
                t(f"{D}{prop}", f'"{lit(v)}"^^<{XSD}{dt}>')
            else:
                t(f"{D}{prop}", f'"{lit(v)}"')
    py = C["publication_year"][i]
    if py and _YEAR.match(str(py).strip()):
        t(f"{D}publicationYear", f'"{str(py).strip()}"^^<{XSD}gYear>')
    for col, prop in [("citation_count", "citationCount"), ("reference_count", "referenceCount"),
                      ("view_count", "viewCount"), ("download_count", "downloadCount")]:
        v = C.get(col, [None] * len(C["doi"]))[i]
        if v not in (None, ""):
            try:
                t(f"{D}{prop}", f'"{int(v)}"^^<{XSD}integer>')
            except Exception:
                pass
    # creators (reified role + agent; orcid-keyed)
    creators = jload(C["creators_json"][i]) or []
    for k, cr in enumerate(creators, 1):
        if not isinstance(cr, dict):
            continue
        orcid = None
        for nid in cr.get("nameIdentifiers", []) or []:
            if isinstance(nid, dict):
                sch = (nid.get("nameIdentifierScheme") or "").lower()
                val = nid.get("nameIdentifier") or ""
                if sch == "orcid" or "orcid.org" in val.lower():
                    m = _ORCID.search(val)
                    if m:
                        orcid = m.group(1)
        role_iri = f"{subj}/creator/{k}"
        airi = (ORCIDB + orcid) if orcid else (role_iri + "/agent")
        t(f"{D}creator", f"<{airi}>")
        buf.append(f"<{subj}> <{D}hasContributor> <{role_iri}> .\n")
        buf.append(f"<{role_iri}> <{RDF_TYPE}> <{D}AgentRole> .\n")
        buf.append(f"<{role_iri}> <{D}isHeldBy> <{airi}> .\n")
        buf.append(f"<{airi}> <{RDF_TYPE}> <{D}Agent> .\n")
        nm = cr.get("name")
        if nm:
            buf.append(f'<{airi}> <{D}agentName> "{lit(nm)}" .\n')
        if orcid:
            buf.append(f'<{airi}> <{D}orcid> "{lit(orcid)}" .\n')
        for aff in cr.get("affiliation", []) or []:
            an = aff.get("name") if isinstance(aff, dict) else aff
            if an:
                buf.append(f'<{airi}> <{D}affiliationName> "{lit(an)}" .\n')
            if isinstance(aff, dict):
                ai = aff.get("affiliationIdentifier") or ""
                if "ror.org" in ai.lower():
                    buf.append(f'<{airi}> <{D}rorId> "{lit(ai)}"^^<{XSD}anyURI> .\n')
    # related identifiers -> typed relation edges
    for rel in jload(C["related_identifiers_json"][i]) or []:
        if not isinstance(rel, dict):
            continue
        if (rel.get("relatedIdentifierType") or "").upper() != "DOI":
            continue
        prop = REL_PROP.get(relkey(rel.get("relationType")))
        rid = rel.get("relatedIdentifier")
        if prop and rid:
            t(f"{D}{prop}", f"<{DOIB}{ienc(rid.strip().lower())}>")
    # funding
    for fr in jload(C["funding_references_json"][i]) or []:
        if not isinstance(fr, dict):
            continue
        fname = fr.get("funderName")
        fid = fr.get("funderIdentifier") or ""
        if "ror.org" in fid.lower():
            firi = RORB + ienc(fid.lower().split("ror.org/")[-1])
        elif fid.lower().startswith("10.") or "doi.org" in fid.lower():
            firi = DOIB + ienc(fid.lower().replace("https://doi.org/", ""))
        elif fname:
            firi = f"{subj}/funder/{ienc(fname)[:80]}"
        else:
            continue
        t(f"{D}fundedBy", f"<{firi}>")
        buf.append(f"<{firi}> <{RDF_TYPE}> <{D}Funder> .\n")
        if fname:
            buf.append(f'<{firi}> <{D}agentName> "{lit(fname)}" .\n')
    # subjects
    for sub in jload(C["subjects_json"][i]) or []:
        s = sub.get("subject") if isinstance(sub, dict) else sub
        if s:
            t(DCT_SUBJECT, f'"{lit(s)}"')


def emit_link(buf, C, i):
    s_id, o_id = C["subj_id"][i], C["obj_id"][i]
    if not s_id or not o_id:
        return
    subj = pid_iri(s_id, C["subj_type"][i])
    obj = pid_iri(o_id, C["obj_type"][i])
    if not subj or not obj:
        return
    rk = relkey(C["relation_type"][i])
    prop = REL_PROP.get(rk, "isRelatedTo")
    buf.append(f"<{subj}> <{D}{prop}> <{obj}> .\n")
    # reified PidRelation (keep source + time — no provenance dropped)
    rel_iri = f"{subj}/rel/{rk}/{ienc(o_id.strip().lower())}"
    buf.append(f"<{rel_iri}> <{RDF_TYPE}> <{D}PidRelation> .\n")
    buf.append(f"<{rel_iri}> <{D}relationSubject> <{subj}> .\n")
    buf.append(f"<{rel_iri}> <{D}relationObject> <{obj}> .\n")
    concept = REL_CONCEPT.get(rk)
    if concept:
        buf.append(f"<{rel_iri}> <{D}relationType> <{D}{concept}> .\n")
    src = C["source_id"][i]
    if src:
        buf.append(f'<{rel_iri}> <{D}assertedBy> <{D}source-{ienc(src)}> .\n')
    occ = C["occurred_at"][i]
    if occ and str(occ).strip():
        buf.append(f'<{rel_iri}> <{D}occurredAt> "{lit(occ)}"^^<{XSD}dateTime> .\n')


META_COLS = ["doi", "prefix", "title", "resource_type", "resource_type_general", "publisher",
             "version", "schema_version", "state", "client_id", "language", "url", "registered",
             "created", "updated", "publication_year", "citation_count", "reference_count",
             "view_count", "download_count", "creators_json", "related_identifiers_json",
             "funding_references_json", "subjects_json"]
LINK_COLS = ["subj_id", "obj_id", "relation_type", "source_id", "subj_type", "obj_type", "occurred_at"]


def run(dirs, cols, emit, out, batch, row_groups):
    n = 0
    for fi, f in enumerate(sorted(sum([glob.glob(os.path.join(d, "*.parquet")) for d in dirs], []))):
        pf = pq.ParquetFile(f)
        avail = [c for c in cols if c in pf.schema_arrow.names]
        if row_groups and fi == 0:
            tbl = pf.read_row_groups(list(range(min(row_groups, pf.num_row_groups))), columns=avail)
            batches = [tbl]
        else:
            batches = pf.iter_batches(batch_size=batch, columns=avail)
        for b in batches:
            C = {c: b.column(c).to_pylist() for c in avail}
            for c in cols:
                C.setdefault(c, [None] * len(C[avail[0]]))
            buf = []
            for i in range(len(C["doi"] if "doi" in C else C["subj_id"])):
                emit(buf, C, i)
                n += 1
                if len(buf) >= 20000:
                    out.write("".join(buf).encode("utf-8")); buf.clear()
            if buf:
                out.write("".join(buf).encode("utf-8"))
        if row_groups:
            break
        print(f"  ...{os.path.basename(f)} done, {n:,} rows", file=sys.stderr, flush=True)
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mode", choices=["all", "metadata", "links"], default="all")
    ap.add_argument("--row-groups", type=int, default=0)
    ap.add_argument("--batch", type=int, default=20000)
    args = ap.parse_args()
    out = sys.stdout.buffer
    meta_dirs = [os.path.join(ROOT, d) for d in ("parquet-2023", "parquet-2024", "parquet-2025")]
    link_dirs = [os.path.join(ROOT, d) for d in ("parquet-links-2023", "parquet-links-may2025")]
    total = 0
    if args.mode in ("all", "metadata"):
        total += run(meta_dirs, META_COLS, emit_metadata, out, args.batch, args.row_groups)
    if args.mode in ("all", "links"):
        total += run(link_dirs, LINK_COLS, emit_link, out, args.batch, args.row_groups)
    print(f"DONE: {total:,} rows emitted", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
