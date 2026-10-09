"""SPARQL 1.1 Protocol endpoints — one standard endpoint per dataset.

``/sparql/{dataset}`` implements the W3C SPARQL 1.1 Protocol (query
operation) so ANY standard client — rdflib/SPARQLWrapper, Jena, YASGUI, a
federated ``SERVICE <…>`` clause in another engine — can talk to every
dataset this gateway publishes. ``{dataset}`` is either a catalog key
(``/sparql/boe``) or a **full range-readable .rete URL** —
``/sparql/https://example.org/any.rete`` — so the gateway is a SPARQL
endpoint for ANY published .rete file, not only its own catalog (raw and
percent-encoded URL forms both work; proxy-collapsed ``https:/`` is
re-normalized):

  * ``GET  /sparql/{dataset}?query=…``
  * ``POST /sparql/{dataset}`` with ``application/x-www-form-urlencoded``
    (``query=…``) or a raw ``application/sparql-query`` body
  * results by content negotiation: SELECT/ASK as
    ``application/sparql-results+json``, CONSTRUCT/DESCRIBE as
    ``application/n-triples`` (a subset of Turtle, so ``text/turtle``
    requests are honored with the same bytes). A quoted triple is written as
    the RDF 1.2 triple term ``<<( s p o )>>``, the default of ``rete export``;
    ``quoted_triple_syntax=rdf-star`` (URL or form parameter) returns rete's
    stored RDF-star surface ``<<s p o>>`` instead. RDF 1.2 has no spelling
    for a quoted triple in subject position, so such a result answers
    ``406`` naming that parameter.
  * FILTER type errors (``CONTAINS`` on an IRI, …) are false by the spec and
    silently drop rows. The engine's warnings about them come back in an
    ``X-Rete-Warnings`` header (a JSON array of messages, ASCII-escaped) on
    every result form, and as a ``warnings`` member of a SPARQL-JSON body.
    Both are absent when there is nothing to report.
  * ``GET`` without ``query`` returns a service description
    (``sd:`` vocabulary, Turtle)

Protocol dataset parameters (``default-graph-uri``/``named-graph-uri``) are
ignored: a ``.rete`` file IS the dataset.
"""
from __future__ import annotations

import json
from typing import Optional

from fastapi import APIRouter, Request, Response

import rete_service as svc

router = APIRouter(tags=["sparql-protocol"])

MIME_SRJ = "application/sparql-results+json"
MIME_NT = "application/n-triples"
MIME_TTL = "text/turtle"
MIME_SPARQL = "application/sparql-query"


def _error(status: int, message: str) -> Response:
    return Response(content=message + "\n", status_code=status, media_type="text/plain")


def _split_source(dataset: str) -> tuple:
    """A path segment is either a catalog key or a full .rete URL.

    Returns ``(dataset, url)`` for :func:`rete_service.run_query`. Proxies
    and path normalization often collapse ``https://`` to ``https:/`` —
    re-expand it (percent-encoded URLs arrive already decoded by Starlette).
    """
    if dataset.startswith(("http://", "https://")):
        return None, dataset
    if dataset.startswith(("http:/", "https:/")):
        scheme, rest = dataset.split(":/", 1)
        return None, f"{scheme}://{rest.lstrip('/')}"
    return dataset, None


def _service_description(request: Request, dataset: str) -> Response:
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


def _respond(dataset: str, query: str, accept: str,
             quoted_triple_syntax: Optional[str] = None) -> Response:
    key, url = _split_source(dataset)
    try:
        doc = svc.run_query(key, url, query,
                            quoted_triple_syntax=quoted_triple_syntax or "rdf12")
    except svc.UnrepresentableResult as e:
        return _error(406, f"{e}\nRetry with quoted_triple_syntax=rdf-star "
                           "to receive the RDF-star surface <<s p o>>.")
    except ValueError as e:
        return _error(404 if "unknown dataset" in str(e) else 400, str(e))
    except TimeoutError as e:
        return _error(504, str(e))
    except Exception as e:
        # Engine parse/eval errors → MalformedQuery per protocol.
        return _error(400, f"{type(e).__name__}: {e}")

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
            # Not part of SPARQL-JSON; standard parsers ignore an extra member.
            payload["warnings"] = warnings
        return Response(content=json.dumps(payload), media_type=MIME_SRJ, headers=headers)
    # CONSTRUCT / DESCRIBE: N-Triples terms, quoted triples already respelled
    # by run_query.
    lines = [" ".join(t[:3]) + " ." for t in (doc.get("triples") or [])]
    media = MIME_TTL if MIME_TTL in (accept or "") else MIME_NT
    return Response(content="\n".join(lines) + ("\n" if lines else ""), media_type=media,
                    headers=headers)


@router.get("/sparql/{dataset:path}")
def sparql_get(dataset: str, request: Request, query: Optional[str] = None):
    """SPARQL 1.1 Protocol query operation (GET form). Without ``query``,
    answers with the endpoint's service description."""
    if query is None:
        return _service_description(request, dataset)
    return _respond(dataset, query, request.headers.get("accept", ""),
                    request.query_params.get("quoted_triple_syntax"))


@router.post("/sparql/{dataset:path}")
async def sparql_post(dataset: str, request: Request):
    """SPARQL 1.1 Protocol query operation (both POST forms)."""
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
    return _respond(dataset, query, request.headers.get("accept", ""), syntax)
