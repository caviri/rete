"""``POST /api/uploads``: a file the person drops on the home page, or that a
script sends with curl, becomes a dataset on this server.

The body is the raw file (not a form), read in chunks and refused as soon as
it passes the cap, so a huge upload never sits in memory. RDF text is built
into a ``.rete`` here; a ``.rete`` is only checked. Either way the file is
kept like every other generated dataset (``generated/<hash>.rete``) and is
queryable through ``/api``, ``/sparql`` and MCP at once. Needs RETE_AUTHOR.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict

import anyio
import rete_graph as rete
from fastapi import APIRouter, HTTPException, Request

from . import state

router = APIRouter(prefix="/api", tags=["uploads"])

# Same suffix table as the graph view (views.RDF_FORMATS), kept here so this
# module stands alone.
RDF_FORMATS = {
    ".nt": "nt", ".ntriples": "nt", ".nq": "nq", ".nquads": "nq", ".ttl": "ttl",
    ".turtle": "ttl", ".trig": "trig", ".rdf": "rdfxml", ".owl": "rdfxml", ".xml": "rdfxml",
    ".jsonld": "jsonld", ".yamlld": "yamlld", ".jelly": "jelly",
}
# RDF text is built in this process, so it has its own, lower ceiling.
RDF_TEXT_MAX = 32 << 20
STARTER = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 20"


def _clean_name(name: str) -> str:
    base = Path(name.replace("\\", "/")).name.strip()
    return base[:120] or "upload"


def _build(name: str, data: bytes) -> bytes:
    """The ``.rete`` bytes for an uploaded file, or HTTPException 400."""
    fmt = RDF_FORMATS.get(Path(name).suffix.lower())
    try:
        if fmt:
            source: Any = data if fmt == "jelly" else data.decode("utf-8")
            data = rete.Builder().add(source, format=fmt).run()
        elif Path(name).suffix.lower() != ".rete":
            raise HTTPException(400, f"{name}: unknown file type. Send RDF ({', '.join(sorted(RDF_FORMATS))}) "
                                     "or a .rete file")
        rete.open(data)
    except HTTPException:
        raise
    except UnicodeDecodeError:
        raise HTTPException(400, f"{name} is not UTF-8 text, as {fmt} must be") from None
    except Exception as e:
        what = f"valid {fmt}" if fmt else "a .rete file this server can read"
        detail = str(e).splitlines()[0] if str(e) else type(e).__name__
        raise HTTPException(400, f"{name} is not {what} ({detail})") from None
    return data


def _store(name: str, data: bytes) -> Dict[str, Any]:
    from . import author

    data = _build(name, data)
    graph = rete.open(data)
    key = f"generated/{author.store_generated(data, name)}"
    try:
        card = state.service.dataset_card(key)
    except Exception:
        card = None
    return {"key": key, "label": name, "bytes": len(data), "statements": graph.quads,
            "card": card, "query": STARTER, "sparql_endpoint": f"/sparql/{key}"}


@router.get("/uploads")
def upload_info():
    """Whether files can be sent here, how large, and which kinds."""
    s = state.settings
    return {"enabled": bool(s.author), "max_mb": s.upload_max_mb,
            "rdf_text_max_mb": min(s.upload_max_mb, RDF_TEXT_MAX >> 20),
            "formats": sorted({*RDF_FORMATS, ".rete"}),
            "note": "" if s.author else "Authoring is off (RETE_AUTHOR=0), so this server stores no uploads."}


@router.post("/uploads")
async def upload(request: Request, name: str):
    """Store the request body as a dataset. ``name`` is the file name: its
    suffix says the format. Answers the dataset key, its card and a starter query.

        curl --data-binary @file.ttl "$URL/api/uploads?name=file.ttl" -H "Authorization: Bearer $RETE_TOKEN"
    """
    s = state.settings
    if not s.author:
        raise HTTPException(403, "uploads are off on this server (authoring is disabled: RETE_AUTHOR=0)")
    name = _clean_name(name)
    suffix = Path(name).suffix.lower()
    if suffix != ".rete" and suffix not in RDF_FORMATS:
        raise HTTPException(400, f"{name}: unknown file type. Send RDF ({', '.join(sorted(RDF_FORMATS))}) "
                                 "or a .rete file")
    cap = s.upload_max_mb << 20
    if suffix != ".rete":
        cap = min(cap, RDF_TEXT_MAX)
    declared = request.headers.get("content-length")
    if declared and declared.isdigit() and int(declared) > cap:
        raise HTTPException(413, f"{name} is larger than the {cap >> 20} MB limit")
    chunks, size = [], 0
    async for chunk in request.stream():
        size += len(chunk)
        if size > cap:
            raise HTTPException(413, f"{name} is larger than the {cap >> 20} MB limit")
        chunks.append(chunk)
    if not size:
        raise HTTPException(400, "the request body is empty: send the file's bytes")
    data = b"".join(chunks)
    del chunks
    try:
        return await anyio.to_thread.run_sync(_store, name, data)
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(state.error_status(e), state.describe_error(e)) from None
