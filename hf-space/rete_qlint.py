"""Deterministic SPARQL query validation and analysis. No LLM involved.

Three inputs, all optional except the query:

* **query alone** — engine parse check + static analysis: form, variables,
  declared vs used prefixes, feature inventory, unsupported constructs,
  hygiene warnings (no LIMIT, ORDER BY expressions…).
* **+ dataset** (catalog key or .rete URL) — vocabulary probes: every
  class/predicate IRI the query uses is ASKed against the graph's index
  (lazy — a handful of range reads), catching the classic silent-0-rows
  mistakes (wrong namespace, typo'd predicate) before execution.
* **+ ontology** (Turtle text) — declaration checks against the ontology
  and subclass counts: "this class has N subclasses; reason=true would
  include them".
"""
from __future__ import annotations

import re
from typing import Any, Dict, List, Optional, Tuple

import rete_graph as rete

import rete_service as svc

# A minimal in-memory graph: parsing is validated by evaluating against it
# (the engine has no separate parse entry point; on one triple, evaluation
# is instant and deterministic).
_PROBE = rete.open(rete.build("<urn:qlint:s> <urn:qlint:p> <urn:qlint:o> ."))

_WELL_KNOWN_PREFIXES = {
    "rdf": "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "owl": "http://www.w3.org/2002/07/owl#",
    "xsd": "http://www.w3.org/2001/XMLSchema#",
}

_FEATURES = [
    ("OPTIONAL", r"\bOPTIONAL\b"), ("UNION", r"\bUNION\b"), ("MINUS", r"\bMINUS\b"),
    ("EXISTS", r"\bEXISTS\b"), ("subquery", r"\{\s*SELECT\b"), ("VALUES", r"\bVALUES\b"),
    ("BIND", r"\bBIND\b"), ("GROUP BY", r"\bGROUP\s+BY\b"), ("HAVING", r"\bHAVING\b"),
    ("ORDER BY", r"\bORDER\s+BY\b"), ("LIMIT", r"\bLIMIT\b"), ("OFFSET", r"\bOFFSET\b"),
    ("SERVICE", r"\bSERVICE\b"), ("GRAPH", r"\bGRAPH\b"), ("FROM", r"\bFROM\b"),
    ("aggregation", r"\b(COUNT|SUM|AVG|MIN|MAX)\s*\("), ("DISTINCT", r"\bDISTINCT\b"),
    ("RDF-star", r"<<"), ("property path (heuristic)", r"(>|\w)[+*]\s|\w\s*/\s*\w+:"),
    ("regex filter", r"\bREGEX\s*\("), ("CONTAINS filter", r"\bCONTAINS\s*\("),
]


def _strip_strings(q: str) -> str:
    return re.sub(r'"""[\s\S]*?"""|"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)*\'', '""', q)


def _prefixes(query: str) -> Tuple[Dict[str, str], List[str], List[str]]:
    """(declared map, used prefix names, undeclared names). Ported from the
    playground gate's declared-prefix lint."""
    stripped = _strip_strings(query)
    no_iris = re.sub(r"<[^>]*>", " ", stripped)
    declared: Dict[str, str] = {}
    for m in re.finditer(r"PREFIX\s+([A-Za-z][\w.\-]*)?:\s*<([^>]*)>", query, re.I):
        declared[(m.group(1) or "").lower()] = m.group(2)
    used = set()
    for m in re.finditer(r"(?:^|[\s(){}\[\]^,;.|/*!=><+])([A-Za-z][\w.\-]*)?:[A-Za-z0-9_%]",
                         no_iris):
        used.add((m.group(1) or "").lower())
    undeclared = sorted(u for u in used if u not in declared)
    return declared, sorted(used), undeclared


def _query_terms(query: str, declared: Dict[str, str]) -> List[Dict[str, str]]:
    """IRIs used in the query, with a coarse role (class if after `a` or
    rdf:type, else predicate/entity)."""
    stripped = _strip_strings(query)
    body = re.sub(r"PREFIX\s+[^>]*>", " ", stripped, flags=re.I)
    terms: Dict[str, str] = {}

    for m in re.finditer(r"\ba\s+<([^>]+)>", body):
        terms[m.group(1)] = "class"
    for m in re.finditer(r"\ba\s+([A-Za-z][\w.\-]*)?:([A-Za-z0-9_.\-]+)", body):
        base = declared.get((m.group(1) or "").lower()) or _WELL_KNOWN_PREFIXES.get(
            (m.group(1) or "").lower())
        if base:
            terms[base + m.group(2)] = "class"
    for m in re.finditer(r"<(https?://[^>]+|urn:[^>]+)>", body):
        terms.setdefault(m.group(1), "iri")
    for m in re.finditer(r"(?:^|[\s;{(\[])([A-Za-z][\w.\-]*)?:([A-Za-z0-9_.\-]+)", body):
        prefix = (m.group(1) or "").lower()
        base = declared.get(prefix) or _WELL_KNOWN_PREFIXES.get(prefix)
        if base:
            terms.setdefault(base + m.group(2), "predicate-or-entity")
    return [{"iri": iri, "role": role} for iri, role in list(terms.items())[:40]]


def validate_query(query: str, dataset: Optional[str] = None, url: Optional[str] = None,
                   ontology: Optional[str] = None) -> Dict[str, Any]:
    """Deterministic validation + analysis of one SPARQL query."""
    doc: Dict[str, Any] = {"ok": True, "errors": [], "warnings": [], "hints": []}

    # 1. Engine parse (evaluated on a one-triple probe graph — instant).
    form = None
    try:
        env = _PROBE.query_raw(query)
        form = env.get("kind")
        doc["variables"] = env.get("vars") or []
    except Exception as e:
        message = str(e)
        if "SERVICE" in query.upper() and "service" in message.lower():
            doc["hints"].append("parse OK; SERVICE cannot be probed offline — "
                                "the endpoint is only contacted on real execution")
        elif "prefix" in message.lower():
            # Name the culprits instead of echoing the raw parser error.
            _, _, undeclared = _prefixes(query)
            doc["ok"] = False
            doc["errors"].append({
                "stage": "prefixes",
                "message": "prefixed names used but never declared: "
                           + (", ".join(undeclared) or "(unknown)")
                           + " — add PREFIX lines (there are no baked-in defaults)",
            })
            return doc
        else:
            doc["ok"] = False
            doc["errors"].append({"stage": "parse", "message": message})
            return doc
    doc["form"] = form

    # 2. Static analysis.
    declared, used, undeclared = _prefixes(query)
    doc["prefixes"] = {"declared": declared, "used": used, "undeclared": undeclared}
    if undeclared:
        doc["ok"] = False
        doc["errors"].append({"stage": "prefixes",
                              "message": f"prefixed names used but never declared: {', '.join(undeclared)}"})
    stripped = _strip_strings(query)
    doc["features"] = [name for name, pat in _FEATURES if re.search(pat, stripped, re.I)]

    if form == "select" and "LIMIT" not in doc["features"]:
        doc["warnings"].append("no LIMIT — fine for aggregates, risky while exploring a lazy remote graph")
    for m in re.finditer(r"ORDER\s+BY\s+(?:ASC|DESC)?\s*\(([^)]*)\)", stripped, re.I):
        if not re.fullmatch(r"\s*\?\w+\s*", m.group(1)):
            doc["warnings"].append("ORDER BY key expressions are not supported (bare variables "
                                   "only) — compute the key in a BIND first")
            break
    if re.search(r"SERVICE\s+\?", stripped, re.I):
        doc["ok"] = False
        doc["errors"].append({"stage": "features",
                              "message": "SERVICE ?var (variable endpoint) is rejected by the engine"})
    if "aggregation" in doc["features"] and "DISTINCT" not in doc["features"]:
        doc["hints"].append("if this runs with reason=true, use COUNT(DISTINCT …) — rewriting "
                            "derives the same instance via several paths")

    terms = _query_terms(query, declared)

    # 3. Vocabulary probes against a real dataset (cheap lazy ASKs).
    if dataset or url:
        source_probes = []
        handle = svc.get_handle(svc.resolve_source(dataset, url)[0])
        with handle.lock:
            for t in terms[:25]:
                iri = t["iri"]
                if t["role"] == "class":
                    present = bool(handle.graph.query_raw(
                        f"ASK {{ ?s a <{iri}> }}").get("boolean"))
                else:
                    present = bool(handle.graph.query_raw(
                        f"ASK {{ {{ ?s <{iri}> ?o }} UNION {{ <{iri}> ?p ?o }} UNION {{ ?s ?p <{iri}> }} }}"
                    ).get("boolean"))
                source_probes.append({**t, "in_dataset": present})
                if not present:
                    doc["warnings"].append(
                        f"{iri} ({t['role']}) matches NOTHING in the dataset — likely a wrong "
                        f"namespace or typo; copy IRIs from the schema profile")
        doc["dataset_probes"] = source_probes

    # 4. Ontology checks (declarations + subclass expansions).
    if ontology:
        try:
            og = rete.open(rete.build(ontology, format="ttl"))
        except Exception as e:
            doc["errors"].append({"stage": "ontology", "message": f"ontology does not parse: {e}"})
            doc["ok"] = False
            return doc
        onto_probes = []
        for t in terms[:25]:
            iri = t["iri"]
            declared_in = bool(og.query_raw(
                f"ASK {{ {{ <{iri}> ?p ?o }} UNION {{ ?s ?p <{iri}> }} }}").get("boolean"))
            entry = {**t, "in_ontology": declared_in}
            if t["role"] == "class":
                n = og.query_raw(
                    "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
                    f"SELECT (COUNT(DISTINCT ?sub) AS ?n) WHERE {{ ?sub rdfs:subClassOf+ <{iri}> }}"
                )["rows"]
                subs = int(rete.Term.parse(n[0]["n"]).value) if n else 0
                entry["subclasses"] = subs
                if subs and "reason" not in query.lower():
                    doc["hints"].append(
                        f"<{iri}> has {subs} subclass(es) in the ontology — run with reason=true "
                        f"to include their instances")
            if not declared_in:
                doc["warnings"].append(f"{iri} is not mentioned in the supplied ontology")
            onto_probes.append(entry)
        doc["ontology_probes"] = onto_probes

    return doc
