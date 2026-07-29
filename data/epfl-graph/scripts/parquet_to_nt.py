"""Stream the EPFL GraphOntology Parquet -> N-Triples (+ optional RDF-star edge
scores) on stdout, following data/epfl-graph/epfl-graph.ttl
(epfl: = https://w3id.org/rete/epflgraph#). Pipe into `rete build -`.

Model (SKOS concept graph):
  Nodes  -> epfl:Concept / epfl:Category / epfl:CuratedArea, with epfl:name,
            depth, referencePageUrl, and is-ontology/is-noise boolean flags
            (emitted only when true).
  Edges  -> epfl:related (Undirected/Symmetric, symmetric), epfl:relatedDirected
            (Directed), epfl:similarTo (the 192.6M Embeddings edges),
            epfl:broader (Category ChildToParent), epfl:anchorPage
            (Category->Concept), epfl:alignedTopic (Category->OpenAlex topic).
            Edge weights attach to the triple with RDF-star:
              << :from epfl:similarTo :to >> epfl:score "0.54"^^xsd:double .
            (--no-scores drops the RDF-star line if the builder can't take it.)

Node IRI = https://w3id.org/rete/epflgraph/n/<id>  (concept numeric id or
category slug; edges reference the same id space).

Usage: python parquet_to_nt.py                 # everything, with scores
       python parquet_to_nt.py --no-scores      # bare edges (no RDF-star)
       python parquet_to_nt.py --only Nodes_N_Concept --limit 5000   # sample
"""
import argparse
import glob
import os
import re
import sys

import pyarrow.parquet as pq

EPFL = "https://w3id.org/rete/epflgraph#"
N = "https://w3id.org/rete/epflgraph/n/"
OATOPIC = "https://openalex.org/T"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD = "http://www.w3.org/2001/XMLSchema#"
PARQ = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "parquet")

_IRI_BAD = re.compile(r'[\x00-\x20<>"{}|\\^`]')
_LIT = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}
_LIT_RE = re.compile(r'[\\"\n\r\t]')


def ienc(x):
    return _IRI_BAD.sub(lambda m: "%%%02X" % ord(m.group()), str(x))


def lit(x):
    s = _LIT_RE.sub(lambda m: _LIT[m.group()], str(x))
    return "".join(c for c in s if ord(c) >= 0x20) if any(ord(c) < 0x20 for c in s) else s


def node(i):
    return f"<{N}{ienc(i)}>"


def is_true(v):
    return str(v).strip() in ("1", "true", "True")


class W:
    def __init__(self, scores):
        self.buf = []
        self.n = 0
        self.scores = scores

    def t(self, s, p, o):
        self.buf.append(f"{s} <{p}> {o} .\n")

    def edge(self, frm, prop, to, score=None):
        s, o = node(frm), node(to)
        self.buf.append(f"{s} <{EPFL}{prop}> {o} .\n")
        if self.scores and score is not None and str(score).strip() not in ("", "None"):
            try:
                sc = float(score)
                self.buf.append(f'<< {s} <{EPFL}{prop}> {o} >> <{EPFL}score> "{sc}"^^<{XSD}double> .\n')
            except ValueError:
                pass
        self.n += 1

    def flush(self, out, force=False):
        if self.buf and (force or len(self.buf) >= 20000):
            out.write("".join(self.buf).encode("utf-8"))
            self.buf.clear()


def cols_of(f):
    return pq.ParquetFile(f).schema_arrow.names


def batches(name, want, batch, limit):
    fs = sorted(glob.glob(os.path.join(PARQ, name, "*.parquet")))
    seen = 0
    for f in fs:
        pf = pq.ParquetFile(f)
        avail = [c for c in want if c in pf.schema_arrow.names]
        if not avail:
            continue
        for b in pf.iter_batches(batch_size=batch, columns=avail):
            d = {c: b.column(c).to_pylist() for c in avail}
            yield d, len(d[avail[0]])
            seen += len(d[avail[0]])
            if limit and seen >= limit:
                return


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--no-scores", action="store_true")
    ap.add_argument("--only", default="")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--batch", type=int, default=100000)
    args = ap.parse_args()
    out = sys.stdout.buffer
    w = W(scores=not args.no_scores)

    def do(name):
        return not args.only or args.only == name

    # ---- nodes ----
    if do("Nodes_N_Concept"):
        for d, k in batches("Nodes_N_Concept", ["id", "name", "is_ontology_concept",
                             "is_ontology_category", "is_noise"], args.batch, args.limit):
            for i in range(k):
                nid = d["id"][i]
                if nid is None:
                    continue
                s = node(nid)
                w.t(s, RDF_TYPE, f"<{EPFL}Concept>")
                if d.get("name", [None]*k)[i]:
                    w.t(s, f"{EPFL}name", f'"{lit(d["name"][i])}"')
                for col, prop in [("is_ontology_concept", "isOntologyConcept"),
                                  ("is_ontology_category", "isOntologyCategory"),
                                  ("is_noise", "isNoise")]:
                    if is_true(d.get(col, [None]*k)[i]):
                        w.t(s, f"{EPFL}{prop}", f'"true"^^<{XSD}boolean>')
                w.n += 1
                w.flush(out)
    if do("Nodes_N_Category"):
        for d, k in batches("Nodes_N_Category", ["id", "name", "depth", "reference_page_url"], args.batch, args.limit):
            for i in range(k):
                if d["id"][i] is None:
                    continue
                s = node(d["id"][i])
                w.t(s, RDF_TYPE, f"<{EPFL}Category>")
                if d.get("name", [None]*k)[i]:
                    w.t(s, f"{EPFL}name", f'"{lit(d["name"][i])}"')
                if d.get("depth", [None]*k)[i] not in (None, ""):
                    try: w.t(s, f"{EPFL}depth", f'"{int(d["depth"][i])}"^^<{XSD}integer>')
                    except (ValueError, TypeError): pass
                if d.get("reference_page_url", [None]*k)[i]:
                    w.t(s, f"{EPFL}referencePageUrl", f'"{lit(d["reference_page_url"][i])}"^^<{XSD}anyURI>')
                w.flush(out)
    if do("Nodes_N_CuratedArea"):
        for d, k in batches("Nodes_N_CuratedArea", ["object_id", "name"], args.batch, args.limit):
            for i in range(k):
                if d.get("object_id", [None]*k)[i] is None:
                    continue
                s = node(d["object_id"][i])
                w.t(s, RDF_TYPE, f"<{EPFL}CuratedArea>")
                if d.get("name", [None]*k)[i]:
                    w.t(s, f"{EPFL}name", f'"{lit(d["name"][i])}"')
                w.flush(out)

    # ---- edges ----
    EDGES = [
        ("Edges_N_Concept_N_Concept_T_Embeddings", "similarTo", True),
        ("Edges_N_Concept_N_Concept_T_Undirected", "related", True),
        ("Edges_N_Concept_N_Concept_T_Symmetric", "related", True),
        ("Edges_N_Concept_N_Concept_T_Directed", "relatedDirected", False),
        ("Edges_N_Category_N_Category_T_ChildToParent", "broader", False),
        ("Edges_N_Category_N_Concept_T_AnchorPage", "anchorPage", False),
        ("Edges_N_ConceptsCluster_N_Concept_T_ParentToChild", "narrower", False),
    ]
    for name, prop, has_score in EDGES:
        if not do(name):
            continue
        want = ["from_id", "to_id"] + (["score"] if has_score else [])
        for d, k in batches(name, want, args.batch, args.limit):
            fr, to = d.get("from_id", [None]*k), d.get("to_id", [None]*k)
            sc = d.get("score", [None]*k)
            for i in range(k):
                if fr[i] is None or to[i] is None:
                    continue
                w.edge(fr[i], prop, to[i], sc[i] if has_score else None)
                w.flush(out)
    # category -> OpenAlex topic alignment
    if do("Edges_N_Category_N_OAlexTopic_T_Semantic"):
        for d, k in batches("Edges_N_Category_N_OAlexTopic_T_Semantic",
                            ["category_id", "topic_id", "score"], args.batch, args.limit):
            for i in range(k):
                cid, tid = d.get("category_id", [None]*k)[i], d.get("topic_id", [None]*k)[i]
                if cid is None or tid is None:
                    continue
                s, o = node(cid), f"<{OATOPIC}{ienc(str(tid).lstrip('T'))}>"
                w.buf.append(f"{s} <{EPFL}alignedTopic> {o} .\n")
                w.n += 1
                w.flush(out)

    w.flush(out, force=True)
    print(f"DONE: {w.n:,} nodes+edges emitted", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()
