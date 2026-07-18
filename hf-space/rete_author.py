"""Authoring tools: let an agent create ontologies and small .rete files.

Three capabilities closing the authoring loop entirely inside one
conversation:

* :func:`suggest_vocabulary` — search Linked Open Vocabularies for existing
  terms before inventing new ones.
* :func:`check_ontology` — parse + profile + a lint battery (pure SPARQL
  over the ontology itself) + an OWL 2 QL rewriter smoke test. Returns
  actionable issues; an agent iterates until clean.
* :func:`build_rete` — assemble a real `.rete` (card, examples, text index)
  and serve it at an ephemeral URL. The file is immediately queryable and
  SHACL-validatable through every existing tool (``dataset="generated/…"``
  or the URL), and range-readable by any rete client or the playground.

Generated files live under ``GENERATED_DIR`` (``/tmp`` by default):
ephemeral by design — download the base64 or republish properly to keep.
"""
from __future__ import annotations

import base64
import hashlib
import os
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

import httpx
import rete_graph as rete

GENERATED_DIR = Path(os.environ.get("RETE_GENERATED_DIR") or "/tmp/rete-generated")
MAX_INPUT_BYTES = int(os.environ.get("AUTHOR_MAX_INPUT_MB") or 4) * (1 << 20)
MAX_GENERATED = int(os.environ.get("AUTHOR_MAX_FILES") or 200)

LOV_API = "https://lov.linkeddata.es/dataset/lov/api/v2/term/search"

_XSD = "http://www.w3.org/2001/XMLSchema#"
_WELL_KNOWN = (
    _XSD, "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#", "http://www.w3.org/2002/07/owl#",
)


# --------------------------------------------------------------------------- #
# Vocabulary discovery
# --------------------------------------------------------------------------- #

def suggest_vocabulary(query: str, limit: int = 8) -> List[Dict[str, Any]]:
    """Search Linked Open Vocabularies (lov.linkeddata.es) for existing terms.

    Returns ``[{term, prefixed, vocabulary, type, score}]`` — reuse these
    IRIs (or subclass them) instead of minting lookalikes.
    """
    r = httpx.get(LOV_API, params={"q": query, "page_size": min(limit, 20)}, timeout=25)
    r.raise_for_status()
    out = []
    for hit in r.json().get("results", []):
        src = hit.get("_source") or hit
        out.append({
            "term": (src.get("uri") or [None])[0] if isinstance(src.get("uri"), list) else src.get("uri"),
            "prefixed": (src.get("prefixedName") or [None])[0] if isinstance(src.get("prefixedName"), list) else src.get("prefixedName"),
            "vocabulary": (src.get("vocabulary.prefix") or [None])[0] if isinstance(src.get("vocabulary.prefix"), list) else src.get("vocabulary.prefix"),
            "type": hit.get("type"),
            "score": hit.get("_score"),
        })
    return out


# --------------------------------------------------------------------------- #
# Ontology checking
# --------------------------------------------------------------------------- #

_PREAMBLE = """
PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX owl:  <http://www.w3.org/2002/07/owl#>
"""

# (severity, message template, SPARQL body binding ?x [and optionally ?y])
_LINTS = [
    ("warning", "{x} is used as rdfs:domain/rdfs:range but never declared in this document (fine only if it is defined in an imported vocabulary)",
     """SELECT DISTINCT ?x WHERE {
          { ?p rdfs:domain ?x } UNION { ?p rdfs:range ?x }
          FILTER(isIRI(?x))
          FILTER NOT EXISTS { ?x a ?anything }
        }"""),
    ("warning", "{x} is the target of rdfs:subClassOf but never declared",
     """SELECT DISTINCT ?x WHERE {
          ?c rdfs:subClassOf ?x . FILTER(isIRI(?x))
          FILTER NOT EXISTS { ?x a ?anything }
        }"""),
    ("warning", "{x} has no rdfs:label — agents and humans will see a bare IRI",
     """SELECT DISTINCT ?x WHERE {
          { ?x a owl:Class } UNION { ?x a rdfs:Class }
          UNION { ?x a owl:ObjectProperty } UNION { ?x a owl:DatatypeProperty }
          FILTER NOT EXISTS { ?x rdfs:label ?l }
        }"""),
    ("info", "{x} has no rdfs:comment — consider documenting the intent",
     """SELECT DISTINCT ?x WHERE {
          { ?x a owl:Class } UNION { ?x a rdfs:Class }
          FILTER NOT EXISTS { ?x rdfs:comment ?c }
        }"""),
    ("info", "property {x} declares no rdfs:domain (harder for agents to place)",
     """SELECT DISTINCT ?x WHERE {
          { ?x a owl:ObjectProperty } UNION { ?x a owl:DatatypeProperty }
          FILTER NOT EXISTS { ?x rdfs:domain ?d }
        }"""),
    ("error", "{x} is declared BOTH owl:ObjectProperty and owl:DatatypeProperty",
     """SELECT DISTINCT ?x WHERE {
          ?x a owl:ObjectProperty . ?x a owl:DatatypeProperty .
        }"""),
    ("warning", "{x} and {y} subclass each other (subClassOf cycle)",
     """SELECT DISTINCT ?x ?y WHERE {
          ?x rdfs:subClassOf ?y . ?y rdfs:subClassOf ?x .
          FILTER(isIRI(?x) && isIRI(?y) && STR(?x) < STR(?y))
        }"""),
]


def check_ontology(ontology_text: str, format: str = "ttl") -> Dict[str, Any]:
    """Validate an ontology draft: parse, profile, lint, reasoning smoke.

    Returns ``{ok, issues: [{severity, message}], profile}``. ``ok`` is
    False on parse errors or any error-severity issue; warnings leave
    ``ok`` True so an agent can decide.
    """
    if len(ontology_text.encode()) > MAX_INPUT_BYTES:
        raise ValueError(f"ontology text exceeds {MAX_INPUT_BYTES} bytes")
    try:
        g = rete.open(rete.build(ontology_text, format=format))
    except Exception as e:
        return {"ok": False, "issues": [{"severity": "error",
                                         "message": f"does not parse as {format}: {e}"}],
                "profile": None}

    issues: List[Dict[str, str]] = []
    for severity, template, body in _LINTS:
        for row in g.query(_PREAMBLE + body):
            values = {k: t.value for k, t in row.items()}
            if severity in ("warning",) and any(
                str(v).startswith(_WELL_KNOWN) for v in values.values()
            ):
                continue  # rdfs:Resource, xsd types etc. are defined elsewhere by definition
            issues.append({"severity": severity, "message": template.format(**values)})

    # Reasoning smoke: the OWL 2 QL rewriter must accept the ontology.
    try:
        g.query("SELECT (COUNT(?c) AS ?n) WHERE { ?c a <http://www.w3.org/2002/07/owl#Class> }",
                reason=True)
        reasoner = "ok"
    except Exception as e:
        reasoner = f"failed: {e}"
        issues.append({"severity": "error", "message": f"OWL 2 QL rewriter rejects the ontology: {e}"})

    classes = g.query(_PREAMBLE + """SELECT (COUNT(DISTINCT ?c) AS ?n) WHERE {
        { ?c a owl:Class } UNION { ?c a rdfs:Class } }""")
    props = g.query(_PREAMBLE + """SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE {
        { ?p a owl:ObjectProperty } UNION { ?p a owl:DatatypeProperty } }""")
    profile = {
        "triples": g.quads,
        "classes": int(classes[0]["n"].value),
        "properties": int(props[0]["n"].value),
        "reasoner": reasoner,
    }
    ok = not any(i["severity"] == "error" for i in issues)
    return {"ok": ok, "issues": issues, "profile": profile}


# --------------------------------------------------------------------------- #
# Building
# --------------------------------------------------------------------------- #

def _evict_generated() -> None:
    files = sorted(GENERATED_DIR.glob("*.rete"), key=lambda p: p.stat().st_mtime)
    while len(files) > MAX_GENERATED:
        files.pop(0).unlink(missing_ok=True)


def build_rete(rdf_text: str, format: str = "ttl",
               card: Optional[Dict[str, Any]] = None,
               examples: Optional[List[Dict[str, str]]] = None,
               text_index: bool = False,
               include_base64: bool = False) -> Dict[str, Any]:
    """Build a real `.rete` from RDF text and serve it at an ephemeral URL.

    ``card`` is the curated Dataset Card (title, description, license, …);
    ``examples`` entries need title/question/sparql. Returns the dataset
    key (queryable immediately via the other tools), the /generated URL
    (range-readable by any rete client and the playground), stats, and —
    when ``include_base64`` — the whole file as a data URI.
    """
    if len(rdf_text.encode()) > MAX_INPUT_BYTES:
        raise ValueError(f"rdf text exceeds {MAX_INPUT_BYTES} bytes")
    builder = rete.Builder().add(rdf_text, format=format)
    if card:
        builder.card(**card)
    for i, ex in enumerate(examples or []):
        builder.example(
            ex["sparql"],
            title=ex.get("title") or f"Example {i + 1}",
            question=ex.get("question") or ex.get("title"),
            dimension=ex.get("dimension") or "custom",
        )
    if text_index:
        builder.text_index(True)
    data = builder.run()

    GENERATED_DIR.mkdir(parents=True, exist_ok=True)
    name = hashlib.sha256(data).hexdigest()[:16] + ".rete"
    path = GENERATED_DIR / name
    path.write_bytes(data)
    _evict_generated()

    g = rete.open(data)
    doc: Dict[str, Any] = {
        "dataset": f"generated/{name}",
        "url": f"/generated/{name}",
        "bytes": len(data),
        "quads": g.quads,
        "terms": g.terms,
        "content_hash": g.content_hash(),
        "note": "ephemeral: survives until the next Space restart — save the "
                "base64 or republish to keep it",
    }
    if include_base64:
        doc["data_uri"] = "data:application/octet-stream;base64," + base64.b64encode(data).decode()
    return doc


def generated_datasets() -> List[Dict[str, Any]]:
    """Catalog entries for the generated files (kind: generated)."""
    out = []
    if GENERATED_DIR.is_dir():
        for p in sorted(GENERATED_DIR.glob("*.rete")):
            out.append({
                "key": f"generated/{p.name}",
                "label": p.stem,
                "description": "Agent-generated .rete (ephemeral).",
                "url": f"/generated/{p.name}",
                "local_path": str(p),
                "size_bytes": p.stat().st_size,
                "kind": "generated",
            })
    return out
