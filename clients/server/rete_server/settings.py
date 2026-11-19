"""Configuration: every option is a command-line flag and a ``RETE_*`` variable.

The environment is read first and flags override it, so a container needs no
flags at all (``docker run -e RETE_TOKEN=…``) and a laptop needs no variables
(``rete-server --token …``).
"""
from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional

# The published catalog: generated from the playground catalog
# (web/playground-src/catalog.js) and uploaded next to the data on R2.
DEFAULT_CATALOG_URL = "https://data.graphplaza.com/catalog.json"
# The copy baked into the package, used when the published one cannot be read.
BUNDLED_CATALOG = Path(__file__).resolve().parent / "catalog.json"


def _split(value: Optional[str]) -> List[str]:
    """``a,b c`` → ``["a", "b", "c"]``: commas or whitespace separate items."""
    if not value:
        return []
    return [item for item in value.replace(",", " ").split() if item]


def _flag(value: Optional[str], default: bool) -> bool:
    if value is None or value.strip() == "":
        return default
    return value.strip().lower() in ("1", "true", "yes", "on")


def read_token_file(path: Path) -> List[str]:
    """One token per line; blank lines and ``#`` comments are ignored."""
    tokens = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            tokens.append(line)
    return tokens


@dataclass
class Settings:
    host: str = "127.0.0.1"
    port: int = 7860
    # Catalog sources, in order: URLs or local JSON files. Empty means only the
    # local folder (and the agent-built files) are served.
    catalogs: List[str] = field(default_factory=lambda: [DEFAULT_CATALOG_URL])
    # A folder of .rete files served as `local/<path>` (each described by its
    # embedded Dataset Card).
    data_dir: Optional[Path] = None
    # Bearer tokens. Any one of them opens every route except /health.
    tokens: List[str] = field(default_factory=list)
    # Also accept `?token=` in the URL, for clients that cannot send a header.
    query_token: bool = False
    # Persistent block cache for remote files (survives restarts when it is on
    # a volume). None reads remote files through the engine's own HTTP reader.
    cache_dir: Optional[Path] = None
    cache_max_mb: int = 2048
    max_handles: int = 12
    row_cap: int = 10_000
    query_timeout_s: float = 60.0
    catalog_refresh_s: float = 600.0
    # Catalog keys to open at startup, so the first query skips the open.
    warm: List[str] = field(default_factory=list)
    # Queries against any http(s) .rete URL, not only catalog entries.
    allow_urls: bool = True
    # Allow `url=` to reach loopback / private / link-local addresses.
    allow_private_urls: bool = False
    # suggest_vocabulary, check_ontology, build_rete, causal_diagram.
    author: bool = True
    generated_dir: Path = field(default_factory=lambda: Path("/tmp/rete-generated"))
    # The largest file a user may open in a graph view (it is held in memory
    # while it arrives); RDF text is also capped at 32 MB, since it is built here.
    upload_max_mb: int = 64
    # pydantic-ai model id enabling /api/ask, e.g. "anthropic:claude-sonnet-5-5".
    ask_model: Optional[str] = None
    # Threads for blocking engine calls (queries release the GIL).
    threads: int = 32
    cors_origins: List[str] = field(default_factory=lambda: ["*"])
    # This server's public base URL, used where an absolute link is needed
    # (the federated example inside a causal_diagram build).
    public_url: Optional[str] = None
    log_level: str = "info"

    @classmethod
    def from_env(cls, env: Optional[dict] = None) -> "Settings":
        e = os.environ if env is None else env
        s = cls()
        s.host = e.get("RETE_HOST") or s.host
        s.port = int(e.get("RETE_PORT") or e.get("PORT") or s.port)
        if "RETE_CATALOG" in e:
            value = e["RETE_CATALOG"].strip()
            s.catalogs = [] if value.lower() in ("", "none", "off") else _split(value)
        if e.get("RETE_DATA_DIR"):
            s.data_dir = Path(e["RETE_DATA_DIR"])
        s.tokens = _split(e.get("RETE_TOKEN"))
        if e.get("RETE_TOKEN_FILE"):
            s.tokens += read_token_file(Path(e["RETE_TOKEN_FILE"]))
        s.query_token = _flag(e.get("RETE_QUERY_TOKEN"), s.query_token)
        if e.get("RETE_CACHE_DIR"):
            s.cache_dir = Path(e["RETE_CACHE_DIR"])
        s.cache_max_mb = int(e.get("RETE_CACHE_MAX_MB") or s.cache_max_mb)
        s.max_handles = int(e.get("RETE_MAX_HANDLES") or s.max_handles)
        s.row_cap = int(e.get("RETE_ROW_CAP") or s.row_cap)
        s.query_timeout_s = float(e.get("RETE_QUERY_TIMEOUT_S") or s.query_timeout_s)
        s.catalog_refresh_s = float(e.get("RETE_CATALOG_REFRESH_S") or s.catalog_refresh_s)
        s.warm = _split(e.get("RETE_WARM"))
        s.allow_urls = _flag(e.get("RETE_ALLOW_URLS"), s.allow_urls)
        s.allow_private_urls = _flag(e.get("RETE_ALLOW_PRIVATE_URLS"), s.allow_private_urls)
        s.author = _flag(e.get("RETE_AUTHOR"), s.author)
        if e.get("RETE_GENERATED_DIR"):
            s.generated_dir = Path(e["RETE_GENERATED_DIR"])
        s.upload_max_mb = int(e.get("RETE_UPLOAD_MAX_MB") or s.upload_max_mb)
        s.ask_model = e.get("RETE_ASK_MODEL") or None
        s.threads = int(e.get("RETE_THREADS") or s.threads)
        if e.get("RETE_CORS_ORIGINS"):
            s.cors_origins = _split(e["RETE_CORS_ORIGINS"])
        s.public_url = (e.get("RETE_PUBLIC_URL") or "").rstrip("/") or None
        s.log_level = e.get("RETE_LOG_LEVEL") or s.log_level
        return s
