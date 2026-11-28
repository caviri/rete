"""REST API: ``/api/*``, documented at ``/docs``.

Every answer that touched a graph echoes ``stats`` (bytes and requests
actually fetched), so how little a lazy query read stays visible.
"""
from __future__ import annotations

from typing import Any, Dict, List, Optional

import orjson
from fastapi import APIRouter, HTTPException, Response
from pydantic import BaseModel, Field

from . import state

router = APIRouter(prefix="/api", tags=["graphs"])
author_router = APIRouter(prefix="/api", tags=["authoring"])


def _call(fn, *args, **kwargs):
    try:
        return fn(*args, **kwargs)
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(state.error_status(e), state.describe_error(e)) from None


# --------------------------------------------------------------------------- #
# Models
# --------------------------------------------------------------------------- #

class DatasetSummary(BaseModel):
    key: str
    label: Optional[str] = None
    description: Optional[str] = None
    url: Optional[str] = Field(None, description="Range-readable .rete URL (usable in any rete client)")
    sparql_endpoint: Optional[str] = Field(None, description="SPARQL 1.1 Protocol endpoint (relative)")
    shards: Optional[List[str]] = Field(None, description="Sharded datasets: query one shard by url")
    triples: Optional[Any] = Field(None, description="Statement count (number or human-readable string)")
    size: Optional[Any] = None
    size_bytes: Optional[int] = None
    license: Optional[str] = None
    source: Optional[str] = None
    tags: Optional[List[str]] = None
    kind: Optional[str] = None


class QueryRequest(BaseModel):
    dataset: Optional[str] = Field(None, description="Catalog key (see GET /api/datasets)")
    url: Optional[str] = Field(None, description="Any public range-readable .rete URL (instead of dataset)")
    query: str = Field(..., description="SPARQL 1.1 SELECT / ASK / CONSTRUCT / DESCRIBE")
    reason: bool = Field(False, description="OWL 2 QL entailment by query rewriting")
    limit: Optional[int] = Field(None, ge=1, description="Row cap for the response (server-capped)")
    quoted_triple_syntax: str = Field(
        "rdf12",
        description="How CONSTRUCT/DESCRIBE write a quoted triple: 'rdf12' (the RDF 1.2 triple "
                    "term <<( s p o )>>, as `rete export` does by default) or 'rdf-star' (<<s p o>>)")


class QueryResult(BaseModel):
    dataset: Optional[str] = None
    kind: str
    elapsed_seconds: float
    stats: Dict[str, Any]
    boolean: Optional[bool] = None
    head: Optional[Dict[str, List[str]]] = None
    results: Optional[Dict[str, Any]] = Field(None, description="W3C SPARQL 1.1 JSON results bindings")
    table: Optional[List[Dict[str, Any]]] = Field(None, description="The same rows as plain values")
    triples: Optional[List[Any]] = None
    truncated: Optional[bool] = None
    warnings: Optional[List[Dict[str, Any]]] = Field(
        None, description="FILTER type errors the engine reported (e.g. CONTAINS on an IRI). A FILTER "
                          "that errors is false, so these explain rows that silently vanished.")


class SearchHit(BaseModel):
    subject: str
    label: Optional[str] = None
    via: str


class ShaclRequest(BaseModel):
    dataset: Optional[str] = Field(None, description="Catalog key")
    url: Optional[str] = Field(None, description="Any public range-readable .rete URL (instead of dataset)")
    shapes: str = Field(..., description="SHACL Core shapes, Turtle")
    graph: Optional[str] = Field(None, description="Validate one named graph instead of the default graph")
    format: str = Field("json", description="'json' (report as object) or 'ttl' (report as Turtle)")


class QueryLintRequest(BaseModel):
    query: str = Field(..., description="The SPARQL query to validate")
    dataset: Optional[str] = Field(None, description="Catalog key to probe vocabulary against")
    url: Optional[str] = Field(None, description="Any public .rete URL (instead of dataset)")
    ontology: Optional[str] = Field(None, description="Ontology (Turtle) to check declarations against")


class AskRequest(BaseModel):
    dataset: str
    question: str = Field(..., description="A natural-language question about the dataset")


# --------------------------------------------------------------------------- #
# Catalog and datasets
# --------------------------------------------------------------------------- #

@router.get("/datasets", response_model=List[DatasetSummary], response_model_exclude_none=True)
def list_datasets():
    """Every dataset this server answers for, with its standard SPARQL 1.1
    Protocol endpoint."""
    out = []
    for entry in state.catalog.entries():
        row = {k: v for k, v in entry.items() if k in DatasetSummary.model_fields}
        if not entry.get("shards"):
            row["sparql_endpoint"] = f"/sparql/{entry['key']}"
        out.append(DatasetSummary(**row))
    return out


@router.get("/datasets/{key:path}/card")
def get_card(key: str):
    """The Dataset Card embedded in the file: title, description, license,
    provenance, counts, example queries. Only the card's byte range is read."""
    card = _call(state.service.dataset_card, key)
    if card is None:
        raise HTTPException(404, f"dataset {key!r} carries no card")
    return card


@router.get("/datasets/{key:path}/schema")
def get_schema(key: str):
    """Classes with instance counts, (subject class, predicate, object class)
    relations, and named graphs."""
    return _call(state.service.dataset_schema, key)


@router.get("/datasets/{key:path}/examples")
def get_examples(key: str):
    """Runnable example queries: the file's own, else the catalog's."""
    return _call(state.service.dataset_examples, key)


@router.get("/datasets/{key:path}/search", response_model=List[SearchHit])
def search_entities(key: str, q: str, limit: int = 20):
    """Entities by label prefix, plus full-text when the file has a text index."""
    return _call(state.service.entity_search, q, key, min(max(limit, 1), 100))


@router.get("/datasets/{key:path}/shapes")
def get_shapes(key: str):
    """Curated SHACL shapes for a dataset; each runs as-is via POST /api/shacl."""
    return _call(state.service.shacl_shapes, key)


@router.get("/datasets/{key:path}")
def get_dataset(key: str):
    """Catalog entry and the file's own card in one response."""
    entry = state.catalog.get(key)
    if entry is None:
        raise HTTPException(404, f"unknown dataset {key!r}")
    doc = state.public_entry(entry)
    if not entry.get("shards"):
        try:
            doc["card"] = state.service.dataset_card(key)
        except Exception as e:
            doc["card"] = None
            doc["card_error"] = state.describe_error(e)
    return doc


# --------------------------------------------------------------------------- #
# Queries
# --------------------------------------------------------------------------- #

def _json(doc: Dict[str, Any]) -> Response:
    # A result is built from engine output already in its final shape;
    # re-validating every row through the response model would cost more
    # than the query on a warm graph. The model still documents it.
    return Response(content=orjson.dumps(doc), media_type="application/json")


@router.post("/query", response_model=None, responses={200: {"model": QueryResult}})
def post_query(req: QueryRequest):
    """Run SPARQL against a dataset (or any public .rete URL). Only the byte
    ranges the query touches are fetched."""
    return _json(_call(state.service.run_query, req.dataset, req.url, req.query, req.reason,
                       req.limit, req.quoted_triple_syntax))


@router.get("/query", response_model=None, responses={200: {"model": QueryResult}})
def get_query(q: str, dataset: Optional[str] = None, url: Optional[str] = None,
              reason: bool = False, limit: Optional[int] = None,
              quoted_triple_syntax: str = "rdf12"):
    """GET form of POST /api/query."""
    return _json(_call(state.service.run_query, dataset, url, q, reason, limit, quoted_triple_syntax))


@router.post("/query/validate")
def validate_query(req: QueryLintRequest):
    """Deterministic query checks: engine parse, prefixes, features, hygiene;
    with a dataset, every class/predicate IRI is probed with a cheap ASK; with
    an ontology, declarations and subclass counts."""
    from . import qlint

    return _call(qlint.validate_query, req.query, req.dataset, req.url, req.ontology)


@router.post("/shacl")
def post_shacl(req: ShaclRequest):
    """Validate against SHACL Core shapes. Over the default graph only the
    shapes' targets are fetched."""
    return _call(state.service.shacl_validate, req.dataset, req.url, req.shapes, req.graph, req.format)


@router.get("/status")
def status():
    """Catalog sources, open graph handles and the disk cache."""
    from . import __version__

    return {"version": __version__, "catalog": state.catalog.status(), **state.service.overview()}


@router.post("/ask")
def ask(req: AskRequest):
    """Natural-language question → SPARQL → answer, through a pydantic-ai
    agent. Needs RETE_ASK_MODEL (and that provider's API key); 503 otherwise."""
    model = state.settings.ask_model
    if not model:
        raise HTTPException(503, "RETE_ASK_MODEL is not configured on this server")
    try:
        from .ask import answer_question  # pydantic-ai is an optional extra
    except ImportError:
        raise HTTPException(503, "install the [ask] extra (pip install \"./clients/server[ask]\") to enable /api/ask") from None
    return _call(answer_question, model, req.dataset, req.question)


# --------------------------------------------------------------------------- #
# Authoring (RETE_AUTHOR)
# --------------------------------------------------------------------------- #

class VocabQuery(BaseModel):
    query: str = Field(..., description="Concept words, e.g. 'argument fallacy premise'")
    limit: int = Field(8, ge=1, le=20)


class OntologyCheck(BaseModel):
    ontology: str = Field(..., description="The ontology draft (Turtle by default)")
    format: str = Field("ttl", description="ttl | nt | rdfxml")


class BuildRequest(BaseModel):
    rdf: str = Field(..., description="RDF text: ontology and/or instances")
    format: str = Field("ttl", description="ttl | nt | nq | rdfxml")
    card: Optional[Dict[str, Any]] = Field(None, description="Dataset Card fields (title, description, license, …)")
    examples: Optional[List[Dict[str, str]]] = Field(None, description="Runnable examples: {title, question, sparql}")
    text_index: bool = False
    include_base64: bool = Field(False, description="Also return the whole file as a data URI")


class CausalClaim(BaseModel):
    cause: str
    effect: str
    relation: str = Field("causes", description="causes | prevents | enables | correlates")
    quote: Optional[str] = Field(None, description="Transcript fragment stating the claim")
    speaker: Optional[str] = None
    confidence: Optional[float] = Field(None, ge=0, le=1)


class CausalRequest(BaseModel):
    claims: List[CausalClaim]
    title: str = "Causal diagram"
    render: str = Field("both", description="mermaid | svg | both")
    build: bool = Field(True, description="Also build a queryable CauseNet-aligned .rete")


@author_router.post("/author/vocabulary")
def author_vocabulary(req: VocabQuery):
    """Search Linked Open Vocabularies for existing terms before minting IRIs."""
    from . import author

    return _call(author.suggest_vocabulary, req.query, req.limit)


@author_router.post("/author/check")
def author_check(req: OntologyCheck):
    """Parse, profile and lint an ontology draft, plus an OWL 2 QL smoke test."""
    from . import author

    return _call(author.check_ontology, req.ontology, req.format)


@author_router.post("/author/build")
def author_build(req: BuildRequest):
    """Build a .rete from RDF text; it is queryable at once as
    `generated/<name>` and downloadable from /generated/<name>."""
    from . import author

    return _call(author.build_rete, req.rdf, req.format, req.card, req.examples,
                 req.text_index, req.include_base64)


@author_router.post("/author/causal")
def author_causal(req: CausalRequest):
    """Causal claims → Mermaid, DOT, a Graphviz SVG, and a CauseNet-aligned .rete."""
    from . import author

    return _call(author.causal_diagram, [c.model_dump() for c in req.claims], req.title,
                 req.render, req.build)
