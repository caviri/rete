"""Shared fixtures: a tiny graph and a Range-capable local HTTP server.

Python's stock ``http.server`` handlers ignore ``Range``, and the engine
(correctly) refuses non-206 answers — so remote tests need this ~30-line
handler implementing the byte-serving subset the format relies on:
HEAD with Content-Length, and single-range GET answered with 206.
"""

from __future__ import annotations

import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest


class _RangeHandler(BaseHTTPRequestHandler):
    payload = b""

    def log_message(self, *args):  # keep pytest output clean
        pass

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(len(self.payload)))
        self.end_headers()

    def do_GET(self):
        data = self.payload
        spec = self.headers.get("Range")
        if spec and spec.startswith("bytes="):
            start_s, end_s = spec[len("bytes=") :].split("-", 1)
            start = int(start_s)
            end = min(int(end_s), len(data) - 1) if end_s else len(data) - 1
            chunk = data[start : end + 1]
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
            self.send_header("Content-Length", str(len(chunk)))
            self.end_headers()
            self.wfile.write(chunk)
        else:
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)


@pytest.fixture
def serve_bytes():
    """Factory: serve a bytes payload over HTTP with Range support; returns
    the URL. ThreadingHTTPServer matters — the client fetches ranges
    concurrently."""
    servers = []

    def _serve(data: bytes) -> str:
        handler = type("Handler", (_RangeHandler,), {"payload": data})
        srv = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        servers.append(srv)
        return f"http://127.0.0.1:{srv.server_address[1]}/graph.rete"

    yield _serve
    for srv in servers:
        srv.shutdown()
        srv.server_close()


NT = """\
<http://example.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .
<http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice \\"the researcher\\"" .
<http://example.org/alice> <http://example.org/age> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://example.org/bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/Person> .
<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob"@en .
<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> .
"""


@pytest.fixture(scope="session")
def nt_text() -> str:
    return NT


@pytest.fixture(scope="session")
def rete_bytes(nt_text) -> bytes:
    import rete_graph as rete

    return rete.build(nt_text)
