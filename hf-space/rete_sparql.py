"""SPARQL 1.1 Protocol endpoints — one standard endpoint per dataset.

``/sparql/{dataset}`` implements the W3C SPARQL 1.1 Protocol (query
operation) so ANY standard client — rdflib/SPARQLWrapper, Jena, YASGUI, a
federated ``SERVICE <…>`` clause in another engine — can talk to every
dataset this gateway publishes:

  * ``GET  /sparql/{dataset}?query=…``
  * ``POST /sparql/{dataset}`` with ``application/x-www-form-urlencoded``
    (``query=…``) or a raw ``application/sparql-query`` body
  * results by content negotiation: SELECT/ASK as
    ``application/sparql-results+json``, CONSTRUCT/DESCRIBE as
    ``application/n-triples`` (a subset of Turtle, so ``text/turtle``
    requests are honored with the same bytes)
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


def _respond(dataset: str, query: str, accept: str) -> Response:
    try:
        doc = svc.run_query(dataset, None, query)
    except ValueError as e:
        return _error(404 if "unknown dataset" in str(e) else 400, str(e))
    except TimeoutError as e:
        return _error(504, str(e))
    except Exception as e:
        # Engine parse/eval errors → MalformedQuery per protocol.
        return _error(400, f"{type(e).__name__}: {e}")

    kind = doc.get("kind")
    if kind == "ask":
        body = json.dumps({"head": {}, "boolean": doc["boolean"]})
        return Response(content=body, media_type=MIME_SRJ)
    if kind in ("select", None):
        body = json.dumps({"head": doc.get("head") or {"vars": []},
                           "results": doc.get("results") or {"bindings": []}})
        return Response(content=body, media_type=MIME_SRJ)
    # CONSTRUCT / DESCRIBE: token triples are already N-Triples terms.
    lines = [" ".join(t) + " ." for t in (doc.get("triples") or [])]
    media = MIME_TTL if MIME_TTL in (accept or "") else MIME_NT
    return Response(content="\n".join(lines) + ("\n" if lines else ""), media_type=media)


@router.get("/sparql/{dataset:path}")
def sparql_get(dataset: str, request: Request, query: Optional[str] = None):
    """SPARQL 1.1 Protocol query operation (GET form). Without ``query``,
    answers with the endpoint's service description."""
    if query is None:
        return _service_description(request, dataset)
    return _respond(dataset, query, request.headers.get("accept", ""))


@router.post("/sparql/{dataset:path}")
async def sparql_post(dataset: str, request: Request):
    """SPARQL 1.1 Protocol query operation (both POST forms)."""
    ctype = (request.headers.get("content-type") or "").split(";")[0].strip()
    if ctype == MIME_SPARQL:
        query = (await request.body()).decode("utf-8", "replace")
    elif ctype in ("application/x-www-form-urlencoded", ""):
        form = await request.form()
        query = form.get("query")
        if not query:
            return _error(400, "missing form parameter: query")
    else:
        return _error(415, f"unsupported content type {ctype!r}; use "
                           f"{MIME_SPARQL} or application/x-www-form-urlencoded")
    return _respond(dataset, query, request.headers.get("accept", ""))
