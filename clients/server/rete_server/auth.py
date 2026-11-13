"""Bearer-token gate in front of every route except ``/health``.

Pure ASGI, so it covers the mounted MCP app and streaming responses alike.

* Clients send ``Authorization: Bearer <token>``. Several tokens may be
  configured at once (rotate by adding the new one, then removing the old).
* ``?token=<token>`` is accepted only when ``query_token`` is on — for clients
  that cannot set a header. The parameter is removed from the request before
  it reaches the app, and so before the access log prints the URL.
* CORS preflights (``OPTIONS``) pass: a browser sends them without
  credentials, and the CORS middleware outside this one answers them.
* Comparison is constant-time.
"""
from __future__ import annotations

import hmac
import json
from typing import Iterable, List
from urllib.parse import parse_qsl, urlencode

OPEN_PATHS = ("/health",)


class TokenAuth:
    def __init__(self, app, tokens: Iterable[str], query_token: bool = False):
        self.app = app
        self.tokens: List[bytes] = [t.encode() for t in tokens if t]
        self.query_token = query_token

    def _accepted(self, candidate: str) -> bool:
        given = candidate.encode()
        ok = False
        for token in self.tokens:  # no early exit: same work for any token
            ok |= hmac.compare_digest(given, token)
        return ok

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http" or not self.tokens:
            return await self.app(scope, receive, send)
        if scope["method"] == "OPTIONS" or scope["path"] in OPEN_PATHS:
            return await self.app(scope, receive, send)

        candidate = None
        for name, value in scope.get("headers") or []:
            if name == b"authorization":
                text = value.decode("latin-1")
                if text[:7].lower() == "bearer ":
                    candidate = text[7:].strip()
                break
        if self.query_token and scope.get("query_string"):
            pairs = parse_qsl(scope["query_string"].decode("latin-1"), keep_blank_values=True)
            kept = [(k, v) for k, v in pairs if k != "token"]
            if len(kept) != len(pairs):
                if candidate is None:
                    candidate = next(v for k, v in pairs if k == "token")
                # In place: the server's access log reads this same scope.
                scope["query_string"] = urlencode(kept).encode("latin-1")

        if candidate is not None and self._accepted(candidate):
            return await self.app(scope, receive, send)

        body = json.dumps({"detail": "missing or invalid token"}).encode()
        await send({
            "type": "http.response.start",
            "status": 401,
            "headers": [
                (b"content-type", b"application/json"),
                (b"content-length", str(len(body)).encode()),
                (b"www-authenticate", b'Bearer realm="rete-server"'),
            ],
        })
        await send({"type": "http.response.body", "body": body})
