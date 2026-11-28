"""``rete-server``: run the web app.

Every flag also exists as a ``RETE_*`` environment variable; a flag wins over
its variable. ``rete-server --help`` lists both.
"""
from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path
from typing import List, Optional

from . import __version__
from .settings import DEFAULT_CATALOG_URL, Settings, read_token_file


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="rete-server",
        description="Serve .rete graph files over REST, the SPARQL 1.1 Protocol and MCP.",
        epilog="Every option can also be set with the RETE_* variable shown in brackets.",
    )
    a = p.add_argument
    a("--version", action="version", version=f"rete-server {__version__}")
    a("--host", help="address to bind (default 127.0.0.1) [RETE_HOST]")
    a("--port", type=int, help="port (default 7860) [RETE_PORT]")
    a("--catalog", action="append", metavar="URL_OR_FILE",
      help=f"a catalog.json to serve; repeat for several. 'none' serves only --data. "
           f"Default: {DEFAULT_CATALOG_URL} [RETE_CATALOG]")
    a("--data", type=Path, metavar="DIR", help="a folder of .rete files to serve as local/<path> [RETE_DATA_DIR]")
    a("--token", action="append", help="bearer token required on every route but /health; "
                                       "repeat for several [RETE_TOKEN, comma-separated]")
    a("--token-file", type=Path, help="file with one token per line (a Docker secret) [RETE_TOKEN_FILE]")
    a("--query-token", action="store_true", default=None,
      help="also accept ?token= in the URL, for clients that cannot send a header [RETE_QUERY_TOKEN]")
    a("--cache-dir", type=Path, help="keep fetched blocks of remote files here (put it on a volume "
                                     "to stay warm across restarts) [RETE_CACHE_DIR]")
    a("--cache-max-mb", type=int, help="disk cache cap (default 2048) [RETE_CACHE_MAX_MB]")
    a("--max-handles", type=int, help="graphs kept open (default 12) [RETE_MAX_HANDLES]")
    a("--row-cap", type=int, help="most rows in one answer (default 10000) [RETE_ROW_CAP]")
    a("--query-timeout", type=float, metavar="SECONDS", help="query time limit (default 60) [RETE_QUERY_TIMEOUT_S]")
    a("--task-timeout", type=float, metavar="SECONDS",
      help="time limit for a long query run as an MCP task (default 900) [RETE_TASK_TIMEOUT_S]")
    a("--task-row-cap", type=int, help="most rows a task may produce (default 100000) [RETE_TASK_ROW_CAP]")
    a("--task-workers", type=int, help="long queries run at once (default 2) [RETE_TASK_WORKERS]")
    a("--inline-rows", type=int, help="rows above which sparql_query stores the answer and returns "
                                      "a summary and a handle (default 200) [RETE_INLINE_ROWS]")
    a("--inline-bytes", type=int, help="the same, in bytes of JSON (default 60000) [RETE_INLINE_BYTES]")
    a("--results-dir", type=Path, help="where stored answers live (default /tmp/rete-results) [RETE_RESULTS_DIR]")
    a("--results-max-mb", type=int, help="stored answers cap, oldest evicted (default 512) [RETE_RESULTS_MAX_MB]")
    a("--results-ttl", type=float, metavar="SECONDS",
      help="how long a stored answer lives (default 3600) [RETE_RESULTS_TTL_S]")
    a("--catalog-refresh", type=float, metavar="SECONDS",
      help="re-read the catalog this often (default 600) [RETE_CATALOG_REFRESH_S]")
    a("--warm", action="append", metavar="KEY", help="open this dataset at startup; repeat [RETE_WARM]")
    a("--no-urls", action="store_true", default=None,
      help="answer only for catalog datasets, never an arbitrary url= [RETE_ALLOW_URLS=0]")
    a("--no-author", action="store_true", default=None,
      help="turn off the authoring tools, /generated and files opened in a graph view [RETE_AUTHOR=0]")
    a("--upload-max-mb", type=int, help="largest file a user may open in a graph view "
                                        "(default 64) [RETE_UPLOAD_MAX_MB]")
    a("--ask-model", help="pydantic-ai model id enabling /api/ask [RETE_ASK_MODEL]")
    a("--public-url", help="this server's public base URL [RETE_PUBLIC_URL]")
    a("--threads", type=int, help="threads for engine calls (default 32) [RETE_THREADS]")
    a("--log-level", choices=["critical", "error", "warning", "info", "debug"], help="[RETE_LOG_LEVEL]")
    return p


def settings_from(argv: Optional[List[str]] = None, env: Optional[dict] = None) -> Settings:
    args = _parser().parse_args(argv)
    s = Settings.from_env(env)
    if args.host:
        s.host = args.host
    if args.port:
        s.port = args.port
    if args.catalog:
        s.catalogs = [] if [c.lower() for c in args.catalog] == ["none"] else list(args.catalog)
    if args.data:
        s.data_dir = args.data
    if args.token:
        s.tokens += [t for t in args.token if t]
    if args.token_file:
        s.tokens += read_token_file(args.token_file)
    if args.query_token:
        s.query_token = True
    if args.cache_dir:
        s.cache_dir = args.cache_dir
    for name, attr in (("cache_max_mb", "cache_max_mb"), ("max_handles", "max_handles"),
                       ("row_cap", "row_cap"), ("query_timeout", "query_timeout_s"),
                       ("catalog_refresh", "catalog_refresh_s"), ("threads", "threads"),
                       ("upload_max_mb", "upload_max_mb"), ("task_timeout", "task_timeout_s"),
                       ("task_row_cap", "task_row_cap"), ("task_workers", "task_workers"),
                       ("inline_rows", "inline_rows"), ("inline_bytes", "inline_bytes"),
                       ("results_dir", "results_dir"), ("results_max_mb", "results_max_mb"),
                       ("results_ttl", "results_ttl_s")):
        value = getattr(args, name)
        if value is not None:
            setattr(s, attr, value)
    if args.warm:
        s.warm = list(args.warm)
    if args.no_urls:
        s.allow_urls = False
    if args.no_author:
        s.author = False
    if args.ask_model:
        s.ask_model = args.ask_model
    if args.public_url:
        s.public_url = args.public_url.rstrip("/")
    if args.log_level:
        s.log_level = args.log_level
    return s


def main(argv: Optional[List[str]] = None) -> int:
    settings = settings_from(argv)
    logging.basicConfig(level=settings.log_level.upper(),
                        format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    if settings.host not in ("127.0.0.1", "localhost", "::1") and not settings.tokens:
        print(f"rete-server: binding {settings.host} with no token — anyone who can reach it "
              "can use it (set RETE_TOKEN or --token)", file=sys.stderr)

    import uvicorn

    from .app import create_app

    app = create_app(settings)
    # One process: every request shares the open graphs and their caches.
    # Threads, not workers, carry the concurrency (queries release the GIL).
    uvicorn.run(app, host=settings.host, port=settings.port, log_level=settings.log_level,
                proxy_headers=True,
                forwarded_allow_ips=os.environ.get("FORWARDED_ALLOW_IPS", "127.0.0.1"),
                timeout_keep_alive=75, limit_concurrency=512)
    return 0
