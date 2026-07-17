"""HTTP Range storage gateway — a project-agnostic static file server.

Serves whatever is under DATA_DIR (default ``/data``) with everything a browser
needs to read remote files lazily, and that a plain object store / HF ``resolve``
endpoint refuses to a cross-origin browser:

  * **HTTP Range** (``206 Partial Content``) — a client faults byte ranges
    instead of downloading whole files,
  * **permissive CORS** (``Access-Control-Allow-Origin: *``, exposing
    ``Content-Range``) — a browser on any origin can read,
  * **HEAD** for cheap length probes.

It is **not tied to any file format** — it serves any HTTP-range-compatible file
(``.rete``, ``.parquet``, ``.duckdb``, images, CSV, anything). The landing page
and links are driven by an optional ``branding.json`` (see ``_branding``); with no
config it shows a generic gateway page.

Routes:
  * ``/``        landing page (HTML, open — no token); themed by branding.json
  * ``/files``   JSON listing of everything served
  * ``/health``  liveness probe (open)
  * ``/logs``    recent access-log entries (gated; only when JWT_TOKEN is set)
  * ``/data/…``  the files themselves (Range/CORS; optional JWT_TOKEN gate)
  * ``/api/…``   SPARQL query plane over the published .rete catalog
                 (datasets, cards, schema, examples, query — see rete_api)
  * ``/mcp``     MCP server (streamable HTTP) exposing the same surface as
                 tools for LLM apps — Claude, ChatGPT connectors (rete_mcp)
"""
import html
import json
import os
import secrets
import time
from contextlib import asynccontextmanager
from pathlib import Path

import anyio
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, JSONResponse, Response
from starlette.responses import StreamingResponse

from rete_api import router as api_router
from rete_mcp import mcp as mcp_server

DATA_DIR = Path(os.environ.get("DATA_DIR") or ("/data" if Path("/data").is_dir() else "data")).resolve()
DATA_DIR.mkdir(parents=True, exist_ok=True)
CHUNK = 1 << 20  # 1 MiB streaming chunk

# How many blocking file-I/O calls may run concurrently in the anyio worker
# threadpool. Starlette runs sync endpoints there; the default cap (40) makes a
# burst of range requests queue behind one another, so we raise it at startup.
THREADPOOL_TOKENS = int(os.environ.get("THREADPOOL_TOKENS") or 200)

# Optional password gate. Set the JWT_TOKEN env var (a deploy Secret) to require
# a matching token on every file request; leave it unset to serve openly.
# Clients pass it as `?token=<JWT_TOKEN>` (browser-friendly — no preflight) or
# `Authorization: Bearer <JWT_TOKEN>`. `/`, `/files`, `/health` stay open;
# `/logs` is only exposed when a token is configured.
AUTH = os.environ.get("JWT_TOKEN") or None

# Internal access log, persisted under DATA_DIR (a dot-prefixed dir so it is
# hidden from the public listing and cannot be served back over /data).
LOG_DIR = DATA_DIR / ".gateway-logs"

# Sensible content types for common payloads. Anything else streams as
# application/octet-stream — the gateway never needs to understand a format.
# Extend at deploy time with EXTRA_CTYPES='{".foo":"x/bar"}'.
_CTYPES = {
    ".json": "application/json", ".csv": "text/csv", ".txt": "text/plain",
    ".rete": "application/octet-stream", ".parquet": "application/octet-stream",
    ".duckdb": "application/octet-stream", ".sqlite": "application/octet-stream",
    ".webp": "image/webp", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".png": "image/png", ".gif": "image/gif", ".svg": "image/svg+xml",
}
try:
    _CTYPES.update(json.loads(os.environ.get("EXTRA_CTYPES") or "{}"))
except Exception:
    pass

# Branding for the landing page. A deploy drops a `branding.json` next to app.py
# (or points BRANDING_FILE at one) to theme the gateway for its project; with no
# file the gateway shows a neutral page. The engine carries no project identity.
DEFAULT_BRANDING = {
    "name": "storage gateway",
    "tagline": "// http range · cors",
    "description": "Serves files with <b>HTTP Range</b> (<code>206 Partial Content</code>) and "
                   "permissive CORS, so a browser on any origin can fetch just the byte ranges it "
                   "needs — lazy reads over gigabyte-scale files, no full download.",
    "accent": "#147d69", "accentDark": "#0b4f42", "accent2": "#c84f2f",
    "links": [],
}


def _branding() -> dict:
    path = Path(os.environ.get("BRANDING_FILE") or (Path(__file__).resolve().parent / "branding.json"))
    try:
        cfg = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        cfg = {}
    b = {**DEFAULT_BRANDING, **cfg}
    b.setdefault("accentDark", b["accent"])
    return b


# MCP over streamable HTTP, stateless so any worker can answer any request.
_mcp_app = mcp_server.http_app(path="/", stateless_http=True)


@asynccontextmanager
async def _lifespan(app: FastAPI):
    """Lift the anyio threadpool cap (concurrent range reads + engine calls
    must not serialize) and run the MCP session manager's lifespan."""
    try:
        anyio.to_thread.current_default_thread_limiter().total_tokens = THREADPOOL_TOKENS
    except Exception:
        pass
    async with _mcp_app.router.lifespan_context(_mcp_app):
        yield


app = FastAPI(title="rete graph gateway", docs_url="/docs", redoc_url=None, lifespan=_lifespan)
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["GET", "HEAD", "POST", "OPTIONS"],
    allow_headers=["Range", "Content-Type", "Authorization", "Mcp-Session-Id", "Mcp-Protocol-Version"],
    expose_headers=["Content-Range", "Accept-Ranges", "Content-Length", "Content-Type", "Mcp-Session-Id"],
    max_age=86400,
)
app.include_router(api_router)
app.mount("/mcp", _mcp_app)


def _is_hidden(rel: str) -> bool:
    """A request path that dips into a dot-prefixed component (e.g. the log dir)."""
    return any(part.startswith(".") for part in Path(rel).parts)


def _log_access(request: Request, status: int, length) -> None:
    """Append one JSON line about a call to the in-storage access log.

    Daily file, O_APPEND of a sub-PIPE_BUF line → safe across workers.
    Best-effort: never let logging break a response.
    """
    try:
        LOG_DIR.mkdir(parents=True, exist_ok=True)
        rec = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "ip": (request.client.host if request.client else None),
            "method": request.method, "path": request.url.path,
            "range": request.headers.get("range"), "status": status, "bytes": length,
            "ua": request.headers.get("user-agent", "")[:120],
        }
        day = time.strftime("%Y-%m-%d", time.gmtime())
        with (LOG_DIR / f"access-{day}.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(rec, separators=(",", ":")) + "\n")
    except Exception:
        pass


def _check_auth(request: Request) -> None:
    """401 unless the request carries the configured token (when AUTH is set)."""
    if not AUTH:
        return
    tok = request.query_params.get("token")
    if not tok:
        h = request.headers.get("authorization", "")
        if h[:7].lower() == "bearer ":
            tok = h[7:].strip()
    if tok != AUTH:
        raise HTTPException(401, "missing or invalid token")


def _resolve(rel: str) -> Path:
    """Resolve a request path under DATA_DIR, rejecting traversal + hidden paths."""
    if _is_hidden(rel):
        raise HTTPException(404, "not found")
    p = (DATA_DIR / rel).resolve()
    if p != DATA_DIR and DATA_DIR not in p.parents:
        raise HTTPException(403, "outside data dir")
    if not p.is_file():
        raise HTTPException(404, "not found")
    return p


def _diagram_svg(accent: str, accent2: str) -> str:
    """Generic illustration of lazy range serving — no project/format specifics."""
    e = html.escape
    return (
        '<svg viewBox="0 0 800 224" role="img" aria-label="A client sends an HTTP Range request and '
        'receives only the bytes it asked for as 206 Partial Content.">'
        '<rect x="24" y="56" width="200" height="112" rx="11" fill="#fff" stroke="#d9e2de"></rect>'
        f'<circle cx="42" cy="74" r="4" fill="{e(accent2)}"></circle>'
        '<circle cx="56" cy="74" r="4" fill="#e0b34a"></circle>'
        f'<circle cx="70" cy="74" r="4" fill="{e(accent)}"></circle>'
        '<line x1="24" y1="88" x2="224" y2="88" stroke="#eef3f1"></line>'
        '<text x="40" y="116" font-family="Cascadia Mono,Consolas,monospace" font-size="12" fill="#17211d">GET /data/file</text>'
        f'<text x="40" y="138" font-family="Cascadia Mono,Consolas,monospace" font-size="12" fill="{e(accent2)}">Range: bytes=4.2M-4.3M</text>'
        '<text x="124" y="160" text-anchor="middle" font-family="sans-serif" font-size="12" fill="#66746e">any client · any origin</text>'
        f'<defs><marker id="ah" markerWidth="9" markerHeight="9" refX="7" refY="4" orient="auto"><path d="M0 0 L8 4 L0 8 z" fill="{e(accent)}"></path></marker>'
        f'<marker id="ah2" markerWidth="9" markerHeight="9" refX="7" refY="4" orient="auto"><path d="M0 0 L8 4 L0 8 z" fill="{e(accent2)}"></path></marker></defs>'
        f'<path d="M228 92 C350 78, 430 78, 552 92" fill="none" stroke="{e(accent)}" stroke-width="2" marker-end="url(#ah)"></path>'
        f'<text x="390" y="74" text-anchor="middle" font-family="Cascadia Mono,Consolas,monospace" font-size="11.5" fill="{e(accent)}">GET · Range: bytes=…</text>'
        f'<path d="M552 138 C430 152, 350 152, 232 138" fill="none" stroke="{e(accent2)}" stroke-width="2" stroke-dasharray="5 4" marker-end="url(#ah2)"></path>'
        f'<text x="390" y="168" text-anchor="middle" font-family="Cascadia Mono,Consolas,monospace" font-size="11.5" fill="{e(accent2)}">206 Partial Content</text>'
        '<rect x="556" y="40" width="220" height="148" rx="11" fill="#fff" stroke="#d9e2de"></rect>'
        + "".join(
            f'<rect x="{574 + col*92}" y="{58 + row*26}" width="84" height="20" rx="3" fill="{accent if (row*2+col) in (1,2,6) else "#eef3f1"}"></rect>'
            for row in range(4) for col in range(2)
        )
        + '<text x="666" y="178" text-anchor="middle" font-family="Cascadia Mono,Consolas,monospace" font-size="11.5" fill="#66746e">file on object storage · serve any range</text>'
        '</svg>'
    )


def _landing_html() -> str:
    """Branding-driven landing page (generic by default). Open, no token."""
    b = _branding()
    e = html.escape
    name, accent, accentDark, accent2 = b["name"], b["accent"], b["accentDark"], b["accent2"]
    cards = "".join(
        f'<a class="card" href="{e(str(l.get("url", "#")))}"><div class="t">{e(str(l.get("title", "")))} '
        f'<span class="arr">→</span></div><div class="d">{e(str(l.get("desc", "")))}</div></a>'
        for l in b.get("links", []) if l.get("title") and l.get("url")
    )
    grid = f'<div class="grid">{cards}</div>' if cards else ""
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{e(name)} — HTTP Range storage gateway</title>
<style>
  :root {{
    --fg:#17211d; --muted:#66746e; --bg:#f6f8f7; --panel:#fff;
    --accent:{accent}; --accent-dark:{accentDark}; --accent-2:{accent2}; --border:#d9e2de;
  }}
  * {{ box-sizing:border-box; }}
  html,body {{ margin:0; }}
  body {{ background:var(--bg); color:var(--fg);
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
    line-height:1.55; -webkit-font-smoothing:antialiased; }}
  .wrap {{ max-width:880px; margin:0 auto; padding:64px 24px 80px; }}
  .brand {{ font-family:Georgia,"Times New Roman",serif; font-size:3.4rem; font-weight:700;
    color:var(--fg); letter-spacing:-.01em; line-height:1; }}
  .brand::after {{ content:""; display:block; width:3.2rem; height:4px; margin-top:.6rem; background:var(--accent-2); }}
  .tag {{ font-family:"Cascadia Mono","SF Mono",Consolas,ui-monospace,monospace; font-size:.8rem;
    text-transform:uppercase; letter-spacing:.18em; color:var(--accent-dark); margin:22px 0 4px; }}
  .lede {{ font-size:1.18rem; color:var(--fg); max-width:60ch; margin:10px 0 6px; }}
  .lede b {{ color:var(--accent-dark); }}
  code {{ font-family:"Cascadia Mono","SF Mono",Consolas,ui-monospace,monospace; font-size:.92em;
    background:#eef3f1; border:1px solid #cfd9d5; border-radius:4px; padding:.05em .35em; }}
  figure.diagram {{ margin:30px 0 6px; padding:18px 18px 12px; background:var(--panel);
    border:1px solid var(--border); border-radius:12px; }}
  figure.diagram svg {{ display:block; width:100%; height:auto; }}
  figure.diagram figcaption {{ margin-top:6px; text-align:center; color:var(--muted);
    font-family:"Cascadia Mono","SF Mono",Consolas,ui-monospace,monospace; font-size:.78rem; }}
  .grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(230px,1fr)); gap:14px; margin:30px 0 8px; }}
  a.card {{ display:block; text-decoration:none; color:var(--fg); background:var(--panel);
    border:1px solid var(--border); border-radius:12px; padding:18px 18px 16px;
    transition:transform .12s ease, box-shadow .12s ease, border-color .12s; }}
  a.card:hover {{ transform:translateY(-3px); border-color:var(--accent);
    box-shadow:0 14px 30px -18px rgba(20,125,105,.5); }}
  a.card .t {{ font-weight:700; font-size:1.05rem; color:var(--accent-dark); }}
  a.card .t .arr {{ color:var(--accent-2); }}
  a.card .d {{ color:var(--muted); font-size:.9rem; margin-top:4px; }}
  .gw {{ margin-top:36px; padding-top:20px; border-top:1px solid var(--border); color:var(--muted); font-size:.92rem; }}
  .gw a {{ color:var(--accent-dark); font-weight:600; text-decoration:none; }}
  .gw a:hover {{ text-decoration:underline; }}
  .dot {{ display:inline-block; width:8px; height:8px; border-radius:50%; background:var(--accent); margin-right:6px; vertical-align:1px; }}
</style>
</head>
<body>
  <div class="wrap">
    <div class="brand">{e(name)}</div>
    <p class="tag">{e(str(b.get("tagline", "")))}</p>
    <p class="lede">{b.get("description", "")}</p>
    <figure class="diagram">
      {_diagram_svg(accent, accent2)}
      <figcaption>// only the bytes a request asks for cross the wire</figcaption>
    </figure>
    {grid}
    <p class="gw"><span class="dot"></span>This host serves <code>/data</code> with HTTP&nbsp;Range and
      permissive CORS — any HTTP-range-compatible file, read lazily. <a href="/health">Health</a></p>
  </div>
</body>
</html>"""


@app.get("/", response_class=HTMLResponse)
def landing():
    """Landing page — open, no token."""
    return HTMLResponse(_landing_html())


@app.get("/files")
def files(request: Request):
    """JSON listing of everything served (hidden dot-paths excluded)."""
    _check_auth(request)
    listing = [
        {"path": str(p.relative_to(DATA_DIR)), "bytes": p.stat().st_size}
        for p in sorted(DATA_DIR.rglob("*"))
        if p.is_file() and not _is_hidden(str(p.relative_to(DATA_DIR)))
    ]
    _log_access(request, 200, None)
    return JSONResponse({"data_dir": str(DATA_DIR), "count": len(listing), "files": listing})


@app.get("/health")
def health():
    return {"ok": True, "data_dir": str(DATA_DIR)}


@app.get("/logs")
def logs(request: Request, n: int = 200):
    """Tail the internal access log. Only exposed when a JWT_TOKEN gate is set."""
    if not AUTH:
        raise HTTPException(404, "not found")
    _check_auth(request)
    day = time.strftime("%Y-%m-%d", time.gmtime())
    path = LOG_DIR / f"access-{day}.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()[-max(1, min(n, 5000)):] if path.is_file() else []
    return JSONResponse({"day": day, "count": len(lines), "entries": [json.loads(x) for x in lines]})


# How many ranges a single multi-range request may carry (abuse guard).
MAX_RANGES = 1000


def _parse_ranges(rng: str, size: int):
    """Parse a Range header (possibly multi-range) into clamped (start, end)
    pairs, or None if any part is unsatisfiable / malformed."""
    unit, _, spec = rng.partition("=")
    if unit.strip() != "bytes":
        return None
    out = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        a, _, b = part.partition("-")
        try:
            if a == "":
                start, end = max(0, size - int(b)), size - 1
            else:
                start, end = int(a), (int(b) if b else size - 1)
        except ValueError:
            return None
        end = min(end, size - 1)
        if start > end or start >= size:
            return None
        out.append((start, end))
    return out or None


def _serve_multirange(p: Path, size: int, ctype: str, base: dict, ranges, is_head: bool):
    """RFC 7233 multipart/byteranges — N ranges in ONE response, so a client can
    coalesce many round trips into a single request (the engine's read_many)."""
    boundary = "rete-" + secrets.token_hex(12)
    part_hdr = lambda s, e: (
        f"--{boundary}\r\nContent-Type: {ctype}\r\n"
        f"Content-Range: bytes {s}-{e}/{size}\r\n\r\n"
    ).encode()
    closing = f"--{boundary}--\r\n".encode()
    total = sum(len(part_hdr(s, e)) + (e - s + 1) + 2 for s, e in ranges) + len(closing)
    media = f"multipart/byteranges; boundary={boundary}"
    headers = {**base, "Content-Length": str(total)}
    if is_head:
        return Response(status_code=206, headers=headers, media_type=media)

    def gen():
        with p.open("rb") as f:
            for s, e in ranges:
                yield part_hdr(s, e)
                f.seek(s)
                remaining = e - s + 1
                while remaining > 0:
                    chunk = f.read(min(CHUNK, remaining))
                    if not chunk:
                        break
                    remaining -= len(chunk)
                    yield chunk
                yield b"\r\n"
            yield closing

    return StreamingResponse(gen(), status_code=206, headers=headers, media_type=media)


@app.head("/data/{rel:path}")
@app.get("/data/{rel:path}")
def serve(rel: str, request: Request):
    _check_auth(request)
    p = _resolve(rel)
    size = p.stat().st_size
    ctype = _CTYPES.get(p.suffix, "application/octet-stream")
    base = {"Accept-Ranges": "bytes", "Cache-Control": "public, max-age=300"}
    is_head = request.method == "HEAD"
    rng = request.headers.get("range")

    if rng:
        ranges = _parse_ranges(rng, size)
        if not ranges or len(ranges) > MAX_RANGES:
            _log_access(request, 416, 0)
            return Response(status_code=416, headers={**base, "Content-Range": f"bytes */{size}"})
        # >= 2 ranges → one multipart/byteranges response (coalesced round trip).
        if len(ranges) >= 2:
            total = sum(e - s + 1 for s, e in ranges)
            _log_access(request, 206, total)
            return _serve_multirange(p, size, ctype, base, ranges, is_head)
        start, end = ranges[0]
        length = end - start + 1
        headers = {**base, "Content-Range": f"bytes {start}-{end}/{size}", "Content-Length": str(length)}
        _log_access(request, 206, length)
        if is_head:
            return Response(status_code=206, headers=headers, media_type=ctype)

        def gen():
            with p.open("rb") as f:
                f.seek(start)
                remaining = length
                while remaining > 0:
                    data = f.read(min(CHUNK, remaining))
                    if not data:
                        break
                    remaining -= len(data)
                    yield data

        return StreamingResponse(gen(), status_code=206, headers=headers, media_type=ctype)

    headers = {**base, "Content-Length": str(size)}
    _log_access(request, 200, size)
    if is_head:
        return Response(status_code=200, headers=headers, media_type=ctype)

    def gen_all():
        with p.open("rb") as f:
            while True:
                data = f.read(CHUNK)
                if not data:
                    break
                yield data

    return StreamingResponse(gen_all(), headers=headers, media_type=ctype)
