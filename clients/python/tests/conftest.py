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


NQ = """\
<http://example.org/alice> <http://www.w3.org/2000/01/rdf-schema#label> "Alice" .
<http://example.org/bob> <http://www.w3.org/2000/01/rdf-schema#label> "Bob" .
<http://example.org/alice> <http://example.org/knows> <http://example.org/bob> <http://example.org/g1> .
<http://example.org/alice> <http://example.org/since> "2019"^^<http://www.w3.org/2001/XMLSchema#gYear> <http://example.org/g1> .
<http://example.org/bob> <http://example.org/knows> <http://example.org/alice> <http://example.org/g2> .
"""


@pytest.fixture(scope="session")
def nq_text() -> str:
    """Two default-graph triples plus two named graphs (2 + 1 quads)."""
    return NQ


@pytest.fixture(scope="session")
def nq_bytes(nq_text) -> bytes:
    import rete_graph as rete

    return rete.build(nq_text, "nq")


#: Triples per subject in the generated fixtures below — enough fan-out that
#: batch boundaries land inside a subject's group as well as between subjects.
FANOUT = 5


def _generated_graph(subjects: int) -> bytes:
    """A ``subjects * FANOUT``-triple graph with long-ish literals, so that
    holding every quad is visibly different from streaming them."""
    import rete_graph as rete

    filler = "x" * 60
    lines = [
        f"<http://example.org/s{s}> <http://example.org/p{i}> "
        f'"value {s}-{i} {filler}" .'
        for s in range(subjects)
        for i in range(FANOUT)
    ]
    return rete.build("\n".join(lines) + "\n")


@pytest.fixture(scope="session")
def big_rete_bytes() -> bytes:
    """40 000 quads (~0.6 MB): too many to be interesting one at a time, few
    enough to walk several times in a test."""
    return _generated_graph(8_000)


@pytest.fixture(scope="session")
def multiblock_rete_bytes() -> bytes:
    """200 000 quads (~3.4 MB): large enough to span many read-cache blocks,
    which is what makes "a ranged dump is not a download" measurable."""
    return _generated_graph(40_000)
