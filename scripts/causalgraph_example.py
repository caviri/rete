#!/usr/bin/env python3
"""A small example causal graph using the causalgraph ontology (Fraunhofer IWU).

The causalgraph repo (github.com/causalgraph/causalgraph, MIT) ships only the
ontology (the TBox) — a library builds instances at runtime. To make it an
explorable rete dataset we author a faithful ABox: a classic Industry-4.0 causal
graph for INJECTION-MOLDING quality, using the ontology's real classes and
properties — Machine/Human Variables, States and Events as CausalNodes; CausalEdges
with hasCause/hasEffect + hasConfidence + hasTimeLag; and Creator provenance
(a process engineer asserted some edges, a causal-discovery algorithm found others).

Writes data/causalgraph/causalgraph-example.nt; merged with the converted ontology
(causalgraph-onto.nt) at build time.
"""
import os

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(HERE, "data", "causalgraph", "causalgraph-example.nt")

CG = "http://iwu.fraunhofer.de/causalgraph#"
EX = "http://iwu.fraunhofer.de/causalgraph/example#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#label"
DBL = "http://www.w3.org/2001/XMLSchema#double"
INT = "http://www.w3.org/2001/XMLSchema#integer"
DATE = "http://www.w3.org/2001/XMLSchema#date"

out = []
def s(subj, pred, obj):  # obj is a full term (<iri> or "lit")
    out.append(f"<{EX}{subj}> <{pred}> {obj} .")
def node(nid, cls, label):
    s(nid, RDF, f"<{CG}{cls}>"); s(nid, RDFS, f'"{label}"')

# ---- CausalNodes -------------------------------------------------------------
node("setpoint",    "HumanInput_Variable", "Operator pressure setpoint")
node("pressure",    "Machine_Variable",    "Injection pressure")
node("melttemp",    "Machine_Variable",    "Melt temperature")
node("coolingtime", "Machine_Variable",    "Cooling time")
node("moldfill",    "Machine_State",       "Mould fill level")
node("warpage",     "Machine_State",       "Part warpage")
node("shortshot",   "Machine_Event",       "Short-shot defect")
node("burnmark",    "Machine_Event",       "Burn-mark defect")

# ---- Creators (provenance) ---------------------------------------------------
node("eng",  "Human_Creator",            "Process engineer (domain knowledge)")
node("algo", "LearningAlgorithm_Creator", "PCMCI causal-discovery run")
s("eng",  "http://purl.org/dc/terms/created", f'"2026-06-22"^^<{DATE}>')
s("algo", "http://purl.org/dc/terms/created", f'"2026-06-22"^^<{DATE}>')

# ---- CausalEdges: (id, cause, effect, confidence, time_lag_s, creator) -------
EDGES = [
    ("e1", "setpoint",    "pressure",  0.98, 0,  "eng",  "operator setpoint drives the press"),
    ("e2", "pressure",    "moldfill",  0.92, 2,  "eng",  "more pressure → fuller mould"),
    ("e3", "pressure",    "shortshot", 0.80, 2,  "algo", "too little pressure → short shot"),
    ("e4", "melttemp",    "burnmark",  0.78, 3,  "algo", "excess melt temperature → burn marks"),
    ("e5", "melttemp",    "moldfill",  0.70, 2,  "algo", "hotter melt flows further"),
    ("e6", "moldfill",    "shortshot", 0.65, 1,  "eng",  "incomplete fill → short shot"),
    ("e7", "coolingtime", "warpage",   0.81, 30, "algo", "short cooling → warpage"),
    ("e8", "pressure",    "warpage",   0.60, 25, "algo", "high holding pressure → residual stress → warpage"),
]
for eid, cause, effect, conf, lag, creator, comment in EDGES:
    s(eid, RDF, f"<{CG}CausalEdge>")
    s(eid, RDFS, f'"{cause} → {effect}"')
    s(eid, "http://www.w3.org/2000/01/rdf-schema#comment", f'"{comment}"')
    s(eid, CG + "hasCause",  f"<{EX}{cause}>")
    s(eid, CG + "hasEffect", f"<{EX}{effect}>")
    s(eid, CG + "hasConfidence", f'"{conf}"^^<{DBL}>')
    s(eid, CG + "hasTimeLag",    f'"{lag}"^^<{INT}>')
    s(eid, CG + "hasCreator",    f"<{EX}{creator}>")
    # node-centric inverses the ontology defines
    s(cause,  CG + "isCausing",    f"<{EX}{eid}>")
    s(effect, CG + "isAffectedBy", f"<{EX}{eid}>")

# ---- the CausalGraph container -----------------------------------------------
node("cg", "CausalGraph", "Injection-moulding quality causal graph")
s("cg", CG + "hasCreator", f"<{EX}eng>")
for nid in ("setpoint", "pressure", "melttemp", "coolingtime", "moldfill", "warpage", "shortshot", "burnmark"):
    s("cg", CG + "hasComponent", f"<{EX}{nid}>")
for eid, *_ in EDGES:
    s("cg", CG + "hasCausalConnection", f"<{EX}{eid}>")

open(OUT, "w", encoding="utf-8").write("\n".join(out) + "\n")
print(f"example: {len(out)} triples ({len(EDGES)} edges, 8 nodes) -> {OUT}")
