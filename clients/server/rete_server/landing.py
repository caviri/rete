"""The home page at ``/``: a static shell (HTML, CSS, JS) with no data in it.

The shell is the one thing served without a token, because a browser cannot
send a header when it navigates. The page asks for the token, keeps it in
``sessionStorage`` and sends it as a bearer token with every ``/api`` call, so
all data stays behind the gate. ``SHELL_PATHS`` is exactly what ``auth.py``
leaves open besides ``/health``.
"""
from __future__ import annotations

from pathlib import Path
from typing import Dict

from fastapi import APIRouter, HTTPException
from fastapi.responses import FileResponse

HOME_DIR = Path(__file__).resolve().parent / "home"
# URL path -> file in HOME_DIR. Nothing else is served, and nothing is globbed.
FILES: Dict[str, str] = {
    "/": "index.html",
    "/home/home.css": "home.css",
    "/home/home.js": "home.js",
    "/home/tools.js": "tools.js",
}
SHELL_PATHS = tuple(FILES)
TYPES = {".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
         ".js": "text/javascript; charset=utf-8"}

router = APIRouter()


def _serve(url_path: str) -> FileResponse:
    name = FILES.get(url_path)
    path = HOME_DIR / name if name else None
    if not path or not path.is_file():
        raise HTTPException(404, "not found")
    return FileResponse(path, media_type=TYPES[path.suffix], headers={"Cache-Control": "no-cache"})


def _route(url_path: str):
    def handler():
        return _serve(url_path)
    handler.__name__ = "home_" + (FILES[url_path].replace(".", "_"))
    router.add_api_route(url_path, handler, methods=["GET"], include_in_schema=False)


for _path in FILES:
    _route(_path)
