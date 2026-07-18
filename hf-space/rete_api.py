"""REST plane: /api/* — pydantic-modeled endpoints over rete_service.

Everything is read-only. Remote datasets are opened lazily and every response
echoes fetch stats, so how little was read stays visible.
"""
from __future__ import annotations

import os
from typing import Any, Dict, List, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

import rete_service as svc

router = APIRouter(prefix="/api", tags=["rete"])


class DatasetSummary(BaseModel):
    key: str
    label: Optional[str] = None
    description: Optional[str] = None
    url: Optional[str] = Field(None, description="Range-readable .rete URL (also usable in any rete client)")
    sparql_endpoint: Optional[str] = Field(None, description="SPARQL 1.1 Protocol endpoint for this dataset (relative)")
    shards: Optional[List[str]] = Field(None, description="Sharded datasets: query one shard by url")
    triples: Optional[Any] = Field(None, description="Triple count (number or human-readable string)")
    size: Optional[Any] = None
    license: Optional[str] = None
    source: Optional[str] = None
    tags: Optional[List[str]] = None
    kind: Optional[str] = None


class QueryRequest(BaseModel):
    dataset: Optional[str] = Field(None, description="Catalog key (see GET /api/datasets)")
    url: Optional[str] = Field(None, description="Any range-readable .rete URL (alternative to dataset)")
    query: str = Field(..., description="SPARQL 1.1 SELECT / ASK / CONSTRUCT / DESCRIBE")
    reason: bool = Field(False, description="Answer with OWL 2 QL entailment by query rewriting")
    limit: Optional[int] = Field(None, ge=1, description="Row cap for the response (server-capped)")


class QueryStats(BaseModel):
    fileLength: Optional[int] = None
    bytes: Optional[int] = None
    requests: Optional[int] = None
    network_requests: Optional[int] = None
    network_bytes: Optional[int] = None
    disk_cache_reads: Optional[int] = None


class QueryResult(BaseModel):
    dataset: Optional[str] = None
    kind: str
    elapsed_seconds: float
    stats: QueryStats
    boolean: Optional[bool] = None
    head: Optional[Dict[str, List[str]]] = None
    results: Optional[Dict[str, Any]] = Field(None, description="W3C SPARQL 1.1 JSON results bindings")
    table: Optional[List[Dict[str, Any]]] = Field(None, description="The same rows as plain Python values")
    triples: Optional[List[Any]] = None
    truncated: Optional[bool] = None


class SearchHit(BaseModel):
    subject: str
    label: Optional[str] = None
    via: str


def _wrap(fn, *args, **kwargs):
    try:
        return fn(*args, **kwargs)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except TimeoutError as e:
        raise HTTPException(504, str(e))
    except Exception as e:  # engine and IO errors surface as text
        raise HTTPException(502, f"{type(e).__name__}: {e}")


@router.get("/datasets", response_model=List[DatasetSummary])
def list_datasets():
    """All published .rete datasets (plus local files under /data), with the
    direct range-readable URL each one is served from and its standard
    SPARQL 1.1 Protocol endpoint."""
    out = []
    for d in svc.load_catalog():
        row = {k: v for k, v in d.items() if k in DatasetSummary.model_fields}
        if not d.get("shards"):
            row["sparql_endpoint"] = f"/sparql/{d['key']}"
        out.append(DatasetSummary(**row))
    return out


@router.get("/datasets/{key:path}/card")
def get_card(key: str):
    """The Dataset Card embedded in the file itself — title, description,
    license, provenance, counts, example queries. Lazy: only the metadata
    section's byte range is fetched."""
    card = _wrap(svc.dataset_card, key)
    if card is None:
        raise HTTPException(404, f"dataset {key!r} carries no card")
    return card


@router.get("/datasets/{key:path}/schema")
def get_schema(key: str):
    """Class and predicate profile read from the file: classes with instance
    counts, subject-class/predicate/object-class relations, named graphs."""
    return _wrap(svc.dataset_schema, key)


@router.get("/datasets/{key:path}/examples")
def get_examples(key: str):
    """Runnable example SPARQL queries — from the file's card when embedded,
    else from the published catalog entry."""
    return _wrap(svc.dataset_examples, key)


@router.get("/datasets/{key:path}/search", response_model=List[SearchHit])
def search_entities(key: str, q: str, limit: int = 20):
    """Find entities by label prefix and (when the file carries a text index)
    full-text word search."""
    return _wrap(svc.entity_search, q, key, min(limit, 100))


@router.get("/datasets/{key:path}/shapes")
def get_shapes(key: str):
    """Curated example SHACL shapes for a dataset (from the published
    catalog) — each entry's `shape` validates as-is via POST /api/shacl."""
    return _wrap(svc.shacl_shapes, key)


class ShaclRequest(BaseModel):
    dataset: Optional[str] = Field(None, description="Catalog key")
    url: Optional[str] = Field(None, description="Any range-readable .rete URL (alternative to dataset)")
    shapes: str = Field(..., description="SHACL Core shapes, Turtle")
    graph: Optional[str] = Field(None, description="Validate one named graph instead of the default graph")
    format: str = Field("json", description="'json' (report as object) or 'ttl' (report as Turtle)")


@router.post("/shacl")
def post_shacl(req: ShaclRequest):
    """Validate a dataset against SHACL Core shapes. Lazy: over the default
    graph only the shapes' targets are fetched — validation reads the index
    in place, never the whole file."""
    return _wrap(svc.shacl_validate, req.dataset, req.url, req.shapes, req.graph, req.format)


@router.get("/datasets/{key:path}")
def get_dataset(key: str):
    """Catalog entry + the file's own card in one response."""
    entry = svc.find_dataset(key)
    if entry is None:
        raise HTTPException(404, f"unknown dataset {key!r}")
    doc = dict(entry)
    if not entry.get("shards"):
        try:
            doc["card"] = svc.dataset_card(key)
        except Exception:
            doc["card"] = None
    return doc


@router.post("/query", response_model=QueryResult, response_model_exclude_none=True)
def post_query(req: QueryRequest):
    """Run SPARQL against one dataset (or any .rete URL). Lazy by default:
    only the byte ranges the query touches are fetched, and fetched blocks
    persist in the disk cache for the next request."""
    return _wrap(svc.run_query, req.dataset, req.url, req.query, req.reason, req.limit)


@router.get("/query", response_model=QueryResult, response_model_exclude_none=True)
def get_query(q: str, dataset: Optional[str] = None, url: Optional[str] = None,
              reason: bool = False, limit: Optional[int] = None):
    """GET convenience form of POST /api/query (curl/browser friendly)."""
    return _wrap(svc.run_query, dataset, url, q, reason, limit)


@router.get("/cache")
def cache_state():
    """Disk-cache occupancy and the currently resident graph handles."""
    return svc.cache_overview()


class EmbedRequest(BaseModel):
    urls: List[str] = Field(..., description="Media URLs to fetch (server-capped list length)")
    max_dimension: int = Field(1024, ge=16, le=4096, description="Long-side cap for recompressed images")
    webp_quality: int = Field(80, ge=1, le=100)


@router.post("/media/embed")
def media_embed(req: EmbedRequest):
    """Fetch media URLs and return base64 data URIs — images recompressed to
    WebP and downscaled, everything else passed through with its MIME.
    Built for generating self-contained HTML."""
    import rete_media
    return _wrap(rete_media.embed_urls, req.urls, req.max_dimension, req.webp_quality)


@router.get("/media/preview")
def media_preview(url: str, max_dimension: int = 512, webp_quality: int = 80):
    """A representative WebP image (data URI) for a media URL: image, PDF
    first page, video frame (lazy over HTTP Range), IIIF info.json/manifest,
    or an HTML page's og:image."""
    import rete_media
    return _wrap(rete_media.preview, url, min(max_dimension, 4096), webp_quality)


class VocabQuery(BaseModel):
    query: str = Field(..., description="Concept words to search for, e.g. 'argument fallacy premise'")
    limit: int = Field(8, ge=1, le=20)


@router.post("/author/vocabulary")
def author_vocabulary(req: VocabQuery):
    """Search Linked Open Vocabularies for existing terms — reuse or
    subclass what exists before minting new IRIs."""
    import rete_author
    return _wrap(rete_author.suggest_vocabulary, req.query, req.limit)


class OntologyCheck(BaseModel):
    ontology: str = Field(..., description="The ontology draft (Turtle by default)")
    format: str = Field("ttl", description="ttl | nt | rdfxml")


@router.post("/author/check")
def author_check(req: OntologyCheck):
    """Validate an ontology draft: parse, profile, lint battery (undeclared
    domains/ranges, dangling subclasses, missing labels, property-type
    clashes, subclass cycles), and an OWL 2 QL rewriter smoke test."""
    import rete_author
    return _wrap(rete_author.check_ontology, req.ontology, req.format)


class BuildRequest(BaseModel):
    rdf: str = Field(..., description="RDF text: ontology and/or instances")
    format: str = Field("ttl", description="ttl | nt | nq | rdfxml")
    card: Optional[Dict[str, Any]] = Field(None, description="Dataset Card fields (title, description, license, …)")
    examples: Optional[List[Dict[str, str]]] = Field(None, description="Runnable examples: {title, question, sparql}")
    text_index: bool = False
    include_base64: bool = Field(False, description="Also return the whole file as a data URI")


@router.post("/author/build")
def author_build(req: BuildRequest):
    """Build a .rete from RDF text and serve it at an ephemeral /generated
    URL — immediately queryable (dataset key or URL) and range-readable by
    any rete client."""
    import rete_author
    return _wrap(rete_author.build_rete, req.rdf, req.format, req.card,
                 req.examples, req.text_index, req.include_base64)


class AskRequest(BaseModel):
    dataset: str
    question: str = Field(..., description="A natural-language question about the dataset")


@router.post("/ask")
def ask(req: AskRequest):
    """Natural-language question → SPARQL → answer, via a pydantic-ai agent.

    Enabled only when ASK_MODEL is configured (e.g. ``anthropic:claude-sonnet-5``
    plus the matching API key env var); otherwise 503.
    """
    model = os.environ.get("ASK_MODEL")
    if not model:
        raise HTTPException(503, "ASK_MODEL is not configured on this deployment")
    from rete_ask import answer_question  # lazy: pydantic-ai import is heavy
    return _wrap(answer_question, model, req.dataset, req.question)
