"""Stream OpenCitations Meta Parquet -> N-Triples on stdout, following the rete
OpenCitations ontology (oc: = https://w3id.org/rete/opencitations#). FULL model:
every column and every value is represented — nothing dropped.

Designed to be PIPED into `rete build - --format nt --memory-budget-mb` so no
huge intermediate .nt ever touches disk.

Per bibliographic resource (one Meta row):
  - subject IRI: https://doi.org/<doi> when a DOI exists (joins DataCite /
    OpenAIRE / DBLP / ORCID by IRI), else native https://w3id.org/oc/meta/<omid>
  - rdf:type -> oc: FaBiO-mapped class from the Meta `type` string
  - oc:omid / oc:doi / oc:pmid / oc:openalex / oc:issn / oc:isbn   (join keys)
  - oc:title, oc:resourceType, oc:publicationYear (gYear),
    oc:publicationDate (xsd:date / gYearMonth), oc:volume, oc:issue, oc:pageRange
  - oc:partOf -> venue node (label + issn(s) + openalex)
  - oc:publishedBy -> publisher agent (name)
  - AUTHORS + EDITORS reified per OCDM: oc:hasContributor -> oc:AgentRole
    (oc:withRole role-author/editor, oc:agentOrder N, oc:isHeldBy -> oc:Agent
    with oc:agentName + oc:orcidId). Agents keyed by ORCID IRI when present
    (joins the ORCID dataset), else the native ra/ OMID.

Agent/venue/publisher metadata is emitted inline (re-emitted per occurrence);
`rete build` deduplicates triples, so no giant in-converter dedup set is needed.

Usage (sample):  python parquet_to_nt.py --row-groups 1 > sample.nt
Usage (full):    python parquet_to_nt.py            # streams everything
"""
import argparse
import glob
import os
import re
import sys

import pyarrow.parquet as pq

OC = "https://w3id.org/rete/opencitations#"
META = "https://w3id.org/oc/meta/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
XSD = "http://www.w3.org/2001/XMLSchema#"

PARQUET_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                           "..", "meta-v13.1.0", "parquet")

TYPE_CLASS = {
    "journal article": "JournalArticle", "book": "Book", "book chapter": "BookChapter",
    "book part": "BookChapter", "book section": "BookChapter", "book series": "BookSeries",
    "proceedings": "Proceedings", "proceedings article": "ProceedingsArticle",
    "journal": "Journal", "dataset": "Dataset", "web content": "WebContent",
    "report": "Report", "preprint": "Preprint", "reference book": "ReferenceBook",
    "reference entry": "ReferenceEntry", "dissertation": "Dissertation",
}

_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')
_ISO_DATE = re.compile(r'^\d{4}-\d{2}-\d{2}$')
_YEAR_MONTH = re.compile(r'^\d{4}-\d{2}$')
_YEAR = re.compile(r'^\d{4}$')
_BRACKET = re.compile(r'^(.*?)\s*\[([^\]]*)\]\s*$')


def iri_enc(s):
    return _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), s)


_LIT = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_LIT_RE = re.compile(r'[\\"\n\r\t]')


def lit(s):
    s = _LIT_RE.sub(lambda m: _LIT[m.group()], s)
    if any(ord(c) < 0x20 for c in s):
        s = "".join(c for c in s if ord(c) >= 0x20)
    return s


def parse_embedded(field):
    """('Display name', {'omid':'ra/..'|'br/..', 'orcid':'..', 'issn':[..], 'openalex':'..', 'doi':'..'})"""
    m = _BRACKET.match(field)
    if not m:
        return field.strip(), {}
    name = m.group(1).strip()
    ids = {}
    for tok in m.group(2).split():
        if ":" not in tok:
            continue
        k, v = tok.split(":", 1)
        if k == "issn":
            ids.setdefault("issn", []).append(v)
        else:
            ids[k] = v
    return name, ids


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--parquet-dir", default=PARQUET_DIR)
    ap.add_argument("--row-groups", type=int, default=0)
    ap.add_argument("--batch", type=int, default=50000)
    args = ap.parse_args()

    files = sorted(glob.glob(os.path.join(args.parquet_dir, "*.parquet")))
    if not files:
        sys.exit(f"no parquet under {args.parquet_dir}")
    out = sys.stdout.buffer
    n = 0

    def agent_block(buf, airi, name, orcid):
        buf.append(f"<{airi}> <{RDF_TYPE}> <{OC}Agent> .\n")
        if name:
            buf.append(f'<{airi}> <{OC}agentName> "{lit(name)}" .\n')
        if orcid:
            buf.append(f'<{airi}> <{OC}orcidId> "{lit(orcid)}" .\n')

    def contributors(buf, subj, resiri_base, field, role, pos_start):
        """Reify each ';'-separated agent as an oc:AgentRole; returns next position."""
        pos = pos_start
        for entry in field.split(";"):
            entry = entry.strip()
            if not entry:
                continue
            pos += 1
            name, ids = parse_embedded(entry)
            orcid = ids.get("orcid")
            om = ids.get("omid")  # ra/....
            role_iri = f"{resiri_base}/ar/{role}/{pos}"
            if orcid:
                airi = "https://orcid.org/" + iri_enc(orcid)
            elif om:
                airi = META + iri_enc(om)
            else:
                airi = role_iri + "/agent"  # only a name — keep it, minted per role
            buf.append(f"<{subj}> <{OC}hasContributor> <{role_iri}> .\n")
            buf.append(f"<{role_iri}> <{RDF_TYPE}> <{OC}AgentRole> .\n")
            buf.append(f"<{role_iri}> <{OC}withRole> <{OC}role-{role}> .\n")
            buf.append(f'<{role_iri}> <{OC}agentOrder> "{pos}"^^<{XSD}integer> .\n')
            buf.append(f"<{role_iri}> <{OC}isHeldBy> <{airi}> .\n")
            agent_block(buf, airi, name, orcid)
        return pos

    def emit_batch(cols):
        nonlocal n
        buf = []
        C = cols
        for i in range(len(C["omid"])):
            om = C["omid"][i]
            if not om:
                continue
            d = C["doi"][i]
            subj = ("https://doi.org/" + iri_enc(d.strip().lower())) if d else (META + iri_enc(om))
            resbase = META + iri_enc(om)  # stable base for minted role IRIs

            def t(p, o):
                buf.append(f"<{subj}> <{p}> {o} .\n")

            ty = C["type"][i]
            cls = TYPE_CLASS.get(ty.strip().lower()) if ty else None
            t(RDF_TYPE, f"<{OC}{cls or 'BibliographicResource'}>")
            t(f"{OC}omid", f'"{lit(om)}"')
            if d:               t(f"{OC}doi", f'"{lit(d.strip().lower())}"')
            if C["pmid"][i]:    t(f"{OC}pmid", f'"{lit(C["pmid"][i])}"')
            if C["openalex"][i]:t(f"{OC}openalex", f'"{lit(C["openalex"][i])}"')
            if C["issn"][i]:    t(f"{OC}issn", f'"{lit(C["issn"][i])}"')
            if C["isbn"][i]:    t(f"{OC}isbn", f'"{lit(C["isbn"][i])}"')
            if C["title"][i]:   t(f"{OC}title", f'"{lit(C["title"][i])}"')
            if ty:              t(f"{OC}resourceType", f'"{lit(ty)}"')
            py = C["pub_year"][i]
            if py and _YEAR.match(str(py).strip()):
                t(f"{OC}publicationYear", f'"{str(py).strip()}"^^<{XSD}gYear>')
            pd = C["pub_date"][i]
            if pd:
                pd = pd.strip()
                if _ISO_DATE.match(pd):
                    t(f"{OC}publicationDate", f'"{pd}"^^<{XSD}date>')
                elif _YEAR_MONTH.match(pd):
                    t(f"{OC}publicationDate", f'"{pd}"^^<{XSD}gYearMonth>')
            if C["volume"][i]: t(f"{OC}volume", f'"{lit(C["volume"][i])}"')
            if C["issue"][i]:  t(f"{OC}issue", f'"{lit(C["issue"][i])}"')
            if C["page"][i]:   t(f"{OC}pageRange", f'"{lit(C["page"][i])}"')
            # venue
            v = C["venue"][i]
            if v:
                vname, vids = parse_embedded(v)
                vom = vids.get("omid")
                if vom:
                    viri = META + iri_enc(vom)
                    t(f"{OC}partOf", f"<{viri}>")
                    buf.append(f"<{viri}> <{RDF_TYPE}> <{OC}BibliographicResource> .\n")
                    buf.append(f'<{viri}> <{OC}omid> "{lit(vom)}" .\n')
                    if vname:
                        buf.append(f'<{viri}> <{RDFS_LABEL}> "{lit(vname)}" .\n')
                    for isn in vids.get("issn", []):
                        buf.append(f'<{viri}> <{OC}issn> "{lit(isn)}" .\n')
                    if vids.get("openalex"):
                        buf.append(f'<{viri}> <{OC}openalex> "{lit(vids["openalex"])}" .\n')
            # publisher
            pb = C["publisher"][i]
            if pb:
                pname, pids = parse_embedded(pb)
                pom = pids.get("omid")
                piri = (META + iri_enc(pom)) if pom else (resbase + "/publisher")
                t(f"{OC}publishedBy", f"<{piri}>")
                buf.append(f"<{piri}> <{RDF_TYPE}> <{OC}Agent> .\n")
                if pname:
                    buf.append(f'<{piri}> <{OC}agentName> "{lit(pname)}" .\n')
            # authors + editors (reified, ordered)
            if C["author"][i]:
                contributors(buf, subj, resbase, C["author"][i], "author", 0)
            if C["editor"][i]:
                contributors(buf, subj, resbase, C["editor"][i], "editor", 0)
            n += 1
            # flush incrementally so peak RAM stays tiny (machine is memory-tight)
            if len(buf) >= 20000:
                out.write("".join(buf).encode("utf-8"))
                buf.clear()
        if buf:
            out.write("".join(buf).encode("utf-8"))

    need = ["omid", "doi", "pmid", "openalex", "issn", "isbn", "pub_year", "title",
            "author", "venue", "volume", "issue", "page", "pub_date", "type",
            "publisher", "editor"]
    for fi, f in enumerate(files):
        pf = pq.ParquetFile(f)
        if args.row_groups and fi == 0:
            tbl = pf.read_row_groups(list(range(min(args.row_groups, pf.num_row_groups))), columns=need)
            emit_batch(tbl.to_pydict())
            break
        for batch in pf.iter_batches(batch_size=args.batch, columns=need):
            emit_batch({c: batch.column(c).to_pylist() for c in need})
        print(f"  ...file {fi+1}/{len(files)} done, {n:,} resources", file=sys.stderr, flush=True)
    print(f"DONE: {n:,} resources emitted", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
