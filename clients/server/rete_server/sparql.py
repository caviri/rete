"""SPARQL 1.1 Protocol: one standard endpoint per dataset.

``/sparql/{dataset}`` implements the W3C query operation, so any standard
client (rdflib/SPARQLWrapper, Jena, YASGUI, a ``SERVICE`` clause in another
engine) can talk to every dataset here. ``{dataset}`` is a catalog key
(``/sparql/boe``) or, when the server allows it, a full public ``.rete`` URL
(``/sparql/https://example.org/any.rete``; a proxy-collapsed ``https:/`` is
re-expanded).

* ``GET ?query=…``; ``POST`` as ``application/x-www-form-urlencoded`` or a raw
  ``application/sparql-query`` body.
* SELECT/ASK answer ``application/sparql-results+json``; CONSTRUCT/DESCRIBE
  answer ``application/n-triples`` (the same bytes for ``text/turtle``). A
  quoted triple is the RDF 1.2 triple term ``<<( s p o )>>``;
  ``quoted_triple_syntax=rdf-star`` returns ``<<s p o>>``. A quoted triple in
  subject position has no RDF 1.2 spelling: 406, naming that parameter.
* FILTER type errors come back in an ``X-Rete-Warnings`` header (a JSON array
  of messages) and as a ``warnings`` member of a SPARQL-JSON body.
* ``GET`` without ``query`` returns a service description (Turtle).

``default-graph-uri``/``named-graph-uri`` are ignored: a ``.rete`` file is the
dataset.
"""
from __future__ import annotations

import json
from typing import Optional

import orjson
from fastapi import APIRouter, Request, Response

from . import state
from .engine import UnknownDataset, UnrepresentableResult

router = APIRouter(tags=["sparql-protocol"])

MIME_SRJ = "application/sparql-results+json"
MIME_NT = "application/n-triples"
MIME_TTL = "text/turtle"
MIME_SPARQL = "application/sparql-query"


def _error(status: int, message: str) -> Response:
    return Response(content=message + "\n", status_code=status, media_type="text/plain")


def _split_source(dataset: str) -> tuple:
    if dataset.startswith(("http://", "https://")):
        return None, dataset
    if dataset.startswith(("http:/", "https:/")):
        scheme, rest = dataset.split(":/", 1)
        return None, f"{scheme}://{rest.lstrip('/')}"
    return dataset, None


def _service_description(request: Request) -> Response:
    endpoint = str(request.url).split("?")[0]
    ttl = f"""@prefix sd: <http://www.w3.org/ns/sparql-service-description#> .
@prefix void: <http://rdfs.org/ns/void#> .

<{endpoint}> a sd:Service ;
    sd:endpoint <{endpoint}> ;
    sd:supportedLanguage sd:SPARQL11Query ;
    sd:resultFormat <http://www.w3.org/ns/formats/SPARQL_Results_JSON> ,
                    <http://www.w3.org/ns/formats/N-Triples> ;
    sd:defaultDataset [ a sd:Dataset ; void:rootResource <{endpoint}> ] .
"""
    return Response(content=ttl, media_type=MIME_TTL)


def _warning_headers(warnings: list) -> dict:
    if not warnings:
        return {}
    messages = [w.get("message") or str(w) for w in warnings]
    return {"X-Rete-Warnings": json.dumps(messages, ensure_ascii=True)}


def _respond(dataset: str, query: str, accept: str, quoted_triple_syntax: Optional[str]) -> Response:
    key, url = _split_source(dataset)
    try:
        doc = state.service.run_query(key, url, query, quoted_triple_syntax=quoted_triple_syntax or "rdf12",
                                      shape="w3c")
    except UnrepresentableResult as e:
        return _error(406, f"{e}\nRetry with quoted_triple_syntax=rdf-star "
                           "to receive the RDF-star surface <<s p o>>.")
    except UnknownDataset as e:
        return _error(404, str(e))
    except Exception as e:
        status = state.error_status(e)
        # Engine errors are MalformedQuery (400) per the protocol.
        return _error(400 if status == 502 else status, state.describe_error(e))

    kind = doc.get("kind")
    warnings = doc.get("warnings") or []
    headers = _warning_headers(warnings)
    if kind in ("ask", "select", None):
        if kind == "ask":
            payload = {"head": {}, "boolean": doc["boolean"]}
        else:
            payload = {"head": doc.get("head") or {"vars": []},
                       "results": doc.get("results") or {"bindings": []}}
        if warnings:
            payload["warnings"] = warnings  # standard parsers ignore an extra member
        return Response(content=orjson.dumps(payload), media_type=MIME_SRJ, headers=headers)
    lines = [" ".join(t[:3]) + " ." for t in (doc.get("triples") or [])]
    media = MIME_TTL if MIME_TTL in (accept or "") else MIME_NT
    return Response(content="\n".join(lines) + ("\n" if lines else ""), media_type=media,
                    headers=headers)


@router.get("/sparql/{dataset:path}")
def sparql_get(dataset: str, request: Request, query: Optional[str] = None):
    """SPARQL 1.1 Protocol query (GET). Without ``query``: the service description."""
    if query is None:
        return _service_description(request)
    return _respond(dataset, query, request.headers.get("accept", ""),
                    request.query_params.get("quoted_triple_syntax"))


@router.post("/sparql/{dataset:path}")
async def sparql_post(dataset: str, request: Request):
    """SPARQL 1.1 Protocol query (both POST forms)."""
    from starlette.concurrency import run_in_threadpool

    ctype = (request.headers.get("content-type") or "").split(";")[0].strip()
    syntax = request.query_params.get("quoted_triple_syntax")
    if ctype == MIME_SPARQL:
        query = (await request.body()).decode("utf-8", "replace")
    elif ctype in ("application/x-www-form-urlencoded", ""):
        form = await request.form()
        query = form.get("query")
        if not query:
            return _error(400, "missing form parameter: query")
        syntax = form.get("quoted_triple_syntax") or syntax
    else:
        return _error(415, f"unsupported content type {ctype!r}; use "
                           f"{MIME_SPARQL} or application/x-www-form-urlencoded")
    # The query blocks; keep it off the event loop.
    return await run_in_threadpool(_respond, dataset, query, request.headers.get("accept", ""), syntax)
