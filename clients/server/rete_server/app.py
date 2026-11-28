"""The ASGI app: REST, SPARQL Protocol and MCP behind one token gate.

``create_app(settings)`` builds it; ``uvicorn --factory rete_server.app:create_app``
runs it with settings from the environment. The routes:

* ``/``            what this server is and how to connect (HTML)
* ``/health``      liveness, the one route a token never gates
* ``/api/…``       REST (``/docs`` documents it)
* ``/sparql/…``    one SPARQL 1.1 Protocol endpoint per dataset
* ``/mcp``         MCP over streamable HTTP
* ``/generated/…`` files built by the authoring tools (when enabled)
"""
from __future__ import annotations

import logging
import threading
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Optional

import anyio
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.middleware.gzip import GZipMiddleware
from fastapi.responses import FileResponse, HTMLResponse

from . import __version__, state
from .api import author_router, router as api_router
from .auth import TokenAuth
from .catalog import Catalog
from .engine import Service
from .landing import landing_html
from .mcp import build_mcp
from .settings import Settings
from .sparql import router as sparql_router

log = logging.getLogger("rete_server")


def create_app(settings: Optional[Settings] = None) -> FastAPI:
    settings = settings or Settings.from_env()
    catalog = Catalog(settings)
    catalog.load()
    service = Service(settings, catalog)
    state.settings, state.catalog, state.service = settings, catalog, service

    # A tool answer is one JSON document, never a stream, so it goes back as
    # plain JSON rather than a one-event SSE stream: measured 33-49% more MCP
    # calls per second and a quarter less CPU per call (bench/).
    mcp_app = build_mcp(settings.author).http_app(
        path="/mcp", stateless_http=True, json_response=True, host_origin_protection=False)

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        # Engine calls block (and release the GIL): give them enough threads.
        anyio.to_thread.current_default_thread_limiter().total_tokens = max(settings.threads, 8)
        catalog.start()
        if settings.warm:
            threading.Thread(target=service.warm, args=(settings.warm,), name="warm", daemon=True).start()
        if not settings.tokens:
            log.warning("no token set: anyone who can reach this server can use it")
        async with mcp_app.lifespan(mcp_app):
            yield
        catalog.stop()

    app = FastAPI(title="rete-server", version=__version__, lifespan=lifespan,
                  docs_url="/docs", redoc_url=None,
                  description="SPARQL, SHACL and MCP over .rete graph files.")

    @app.get("/", response_class=HTMLResponse, include_in_schema=False)
    def landing(request: Request):
        base = settings.public_url or str(request.base_url).rstrip("/")
        return HTMLResponse(landing_html(base, __version__, catalog.entries(), bool(settings.tokens),
                                         settings.author, bool(settings.ask_model)))

    @app.get("/health")
    def health():
        """Liveness. Open even when a token is set (container health checks)."""
        return {"ok": True, "version": __version__, "datasets": len(catalog.keys())}

    app.include_router(api_router)
    app.include_router(sparql_router)
    if settings.author:
        app.include_router(author_router)

        def generated(name: str):
            """A file built by build_rete: range-readable by any rete client."""
            if "/" in name or "\\" in name or name.startswith(".") or not name.endswith(".rete"):
                raise HTTPException(404, "not found")
            path: Path = settings.generated_dir / name
            if not path.is_file():
                raise HTTPException(404, "not found (generated files are ephemeral)")
            return FileResponse(path, media_type="application/octet-stream",
                                headers={"Cache-Control": "no-store"})

        app.add_api_route("/generated/{name}", generated, methods=["GET"], tags=["authoring"])
        app.add_api_route("/generated/{name}", generated, methods=["HEAD"], include_in_schema=False)

    # Last, so every route above wins; the MCP app answers /mcp.
    app.mount("/", mcp_app)

    # Outermost last: CORS wraps the token gate, so a 401 still carries CORS
    # headers and preflights are answered without a token.
    # Level 4: most of level 9's ratio on JSON for a fraction of the CPU.
    app.add_middleware(GZipMiddleware, minimum_size=2048, compresslevel=4)
    app.add_middleware(TokenAuth, tokens=settings.tokens, query_token=settings.query_token)
    app.add_middleware(
        CORSMiddleware,
        allow_origins=settings.cors_origins,
        allow_methods=["GET", "HEAD", "POST", "OPTIONS"],
        allow_headers=["Authorization", "Content-Type", "Range", "Accept",
                       "Mcp-Session-Id", "Mcp-Protocol-Version", "Last-Event-ID"],
        expose_headers=["Content-Range", "Accept-Ranges", "Content-Length", "Content-Type",
                        "Mcp-Session-Id", "X-Rete-Warnings", "WWW-Authenticate"],
        max_age=86400,
    )
    return app
