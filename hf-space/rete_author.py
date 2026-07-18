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


# --------------------------------------------------------------------------- #
# Causal diagrams from conversations
# --------------------------------------------------------------------------- #

# Aligned with CauseNet (the 500M-claim causal KG in the catalog): our claim
# class subclasses cn:CausalRelation and reuses cn:cause/cn:effect, so a
# conversation's causal graph federates with the web's causal knowledge.
CAUSAL_ONTOLOGY = """@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl:  <http://www.w3.org/2002/07/owl#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix cn:   <https://causenet.org/ontology#> .
@prefix cz:   <https://w3id.org/rete/causal-conv#> .

cn:CausalRelation a owl:Class ; rdfs:label "CauseNet causal relation" ;
  rdfs:comment "Imported anchor: the CauseNet relation class." .
cn:Concept a owl:Class ; rdfs:label "CauseNet concept" .

cz:Factor a owl:Class ; rdfs:subClassOf cn:Concept ; rdfs:label "Factor" ;
  rdfs:comment "A variable or phenomenon mentioned in the conversation." .
cz:Claim a owl:Class ; rdfs:subClassOf cn:CausalRelation ; rdfs:label "Causal claim" ;
  rdfs:comment "One cause-effect assertion made in the conversation." .

cz:relation a owl:DatatypeProperty ; rdfs:domain cz:Claim ; rdfs:range xsd:string ;
  rdfs:label "relation kind" ; rdfs:comment "causes | prevents | enables | correlates" .
cz:polarity a owl:DatatypeProperty ; rdfs:domain cz:Claim ; rdfs:range xsd:string ;
  rdfs:label "polarity" .
cz:quote a owl:DatatypeProperty ; rdfs:domain cz:Claim ; rdfs:range xsd:string ;
  rdfs:label "quote" ; rdfs:comment "The transcript fragment stating the claim." .
cz:statedBy a owl:DatatypeProperty ; rdfs:domain cz:Claim ; rdfs:range xsd:string ;
  rdfs:label "stated by" .
cz:confidence a owl:DatatypeProperty ; rdfs:domain cz:Claim ; rdfs:range xsd:decimal ;
  rdfs:label "confidence" .
"""

_RELATIONS = ("causes", "prevents", "enables", "correlates")


def _slug(text: str) -> str:
    out = "".join(ch if ch.isalnum() else "-" for ch in text.lower().strip())
    return "-".join(p for p in out.split("-") if p)[:64] or "factor"


def _ttl_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ")


def causal_diagram(claims: List[Dict[str, Any]], title: str = "Causal diagram",
                   render: str = "both", build: bool = True) -> Dict[str, Any]:
    """Turn extracted causal claims into a diagram + a queryable graph.

    Each claim: ``{cause, effect, relation?, quote?, speaker?, confidence?}``
    with relation in causes|prevents|enables|correlates. Returns Mermaid and
    DOT sources always; ``render`` svg|both adds a Graphviz-rendered SVG
    data URI; ``build`` also assembles a CauseNet-aligned `.rete` (served,
    immediately queryable, federable with the `causenet` dataset).
    """
    if not claims:
        raise ValueError("claims must be a non-empty list")
    if len(claims) > 200:
        raise ValueError("at most 200 claims per diagram")

    factors: Dict[str, str] = {}
    edges = []
    for i, c in enumerate(claims):
        cause, effect = str(c.get("cause", "")).strip(), str(c.get("effect", "")).strip()
        if not cause or not effect:
            raise ValueError(f"claim {i} needs both cause and effect")
        relation = str(c.get("relation") or "causes").lower()
        if relation not in _RELATIONS:
            raise ValueError(f"claim {i}: relation must be one of {_RELATIONS}")
        for label in (cause, effect):
            factors.setdefault(_slug(label), label)
        edges.append({"n": i + 1, "cause": _slug(cause), "effect": _slug(effect),
                      "relation": relation, "quote": c.get("quote"),
                      "speaker": c.get("speaker"), "confidence": c.get("confidence")})

    # Mermaid (renders natively in chat UIs and the docs).
    arrows = {"causes": "-->|causes|", "prevents": "-.->|prevents|",
              "enables": "-->|enables|", "correlates": "---|correlates|"}
    lines = ["flowchart LR"]
    for slug, label in factors.items():
        lines.append(f'  {slug}["{label}"]')
    for e in edges:
        lines.append(f"  {e['cause']} {arrows[e['relation']]} {e['effect']}")
    mermaid = "\n".join(lines)

    # DOT (Graphviz).
    styles = {"causes": "", "prevents": ' color="firebrick" arrowhead=tee',
              "enables": ' style=dashed', "correlates": ' dir=none style=dotted'}
    dot_lines = ["digraph causal {", '  rankdir=LR; node [shape=box, style="rounded,filled", '
                 'fillcolor="#eef3f1", fontname="Helvetica"]; edge [fontname="Helvetica", fontsize=11];']
    for slug, label in factors.items():
        dot_lines.append(f'  "{slug}" [label="{_ttl_escape(label)}"];')
    for e in edges:
        dot_lines.append(f'  "{e["cause"]}" -> "{e["effect"]}" [label="{e["relation"]}"{styles[e["relation"]]}];')
    dot_lines.append("}")
    dot = "\n".join(dot_lines)

    doc: Dict[str, Any] = {"title": title, "factors": len(factors),
                           "claims": len(edges), "mermaid": mermaid, "dot": dot}

    if render in ("svg", "both"):
        import subprocess
        try:
            svg = subprocess.run(["dot", "-Tsvg"], input=dot.encode(),
                                 capture_output=True, check=True, timeout=30).stdout
            doc["svg_data_uri"] = "data:image/svg+xml;base64," + base64.b64encode(svg).decode()
        except Exception as e:
            doc["svg_error"] = f"graphviz render failed: {e}"

    if build:
        ns = "urn:causal:" + _slug(title) + "#"
        ttl = [f"@prefix cz: <https://w3id.org/rete/causal-conv#> .",
               f"@prefix cn: <https://causenet.org/ontology#> .",
               f"@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .",
               f"@prefix xsd: <http://www.w3.org/2001/XMLSchema#> ."]
        for slug, label in factors.items():
            ttl.append(f'<{ns}{slug}> a cz:Factor ; rdfs:label "{_ttl_escape(label)}" .')
        for e in edges:
            parts = [f'<{ns}claim-{e["n"]}> a cz:Claim',
                     f'cn:cause <{ns}{e["cause"]}>', f'cn:effect <{ns}{e["effect"]}>',
                     f'cz:relation "{e["relation"]}"']
            if e.get("quote"):
                parts.append(f'cz:quote "{_ttl_escape(str(e["quote"]))}"')
            if e.get("speaker"):
                parts.append(f'cz:statedBy "{_ttl_escape(str(e["speaker"]))}"')
            if e.get("confidence") is not None:
                parts.append(f'cz:confidence "{float(e["confidence"])}"^^xsd:decimal')
            ttl.append(" ; ".join(parts) + " .")
        federated_example = (
            "PREFIX cn: <https://causenet.org/ontology#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
            "SELECT ?factor ?webCause WHERE {\n  ?f a <https://w3id.org/rete/causal-conv#Factor> ; rdfs:label ?factor .\n"
            "  SERVICE <https://katospiegel-rete.hf.space/sparql/causenet> {\n"
            "    ?rel cn:effect ?c . ?c rdfs:label ?factor .\n    ?rel cn:cause ?wc . ?wc rdfs:label ?webCause .\n  }\n} LIMIT 20"
        )
        built = build_rete(
            CAUSAL_ONTOLOGY + "\n" + "\n".join(ttl),
            card={"title": title,
                  "description": "Causal claims extracted from a conversation, "
                                 "CauseNet-aligned (cn:cause/cn:effect).",
                  "license": "CC0-1.0"},
            examples=[
                {"title": "All causal claims",
                 "question": "What causes what, according to the conversation?",
                 "sparql": "PREFIX cn: <https://causenet.org/ontology#>\nPREFIX cz: <https://w3id.org/rete/causal-conv#>\n"
                           "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"
                           "SELECT ?cause ?relation ?effect ?speaker WHERE {\n  ?cl a cz:Claim ; cn:cause ?c ; cn:effect ?e ; cz:relation ?relation .\n"
                           "  ?c rdfs:label ?cause . ?e rdfs:label ?effect .\n  OPTIONAL { ?cl cz:statedBy ?speaker }\n}"},
                {"title": "Does the web agree? (federated with CauseNet)",
                 "question": "Which conversation factors have known causes in CauseNet?",
                 "sparql": federated_example},
            ],
        )
        doc.update({k: built[k] for k in ("dataset", "url", "bytes", "quads")})
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
