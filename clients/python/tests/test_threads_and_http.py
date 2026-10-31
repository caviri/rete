"""The native wheel's threading and HTTP contract (0.4.2).

* It is built with rete-core's ``parallel`` (rayon) and ``http`` features, and a
  parallel build writes the same bytes as a single-threaded one.
* Engine calls release the GIL: another Python thread runs while a build or a
  query is in Rust.
* ``open(url)`` uses the CLI's HTTP reader, so a host that ignores ``Range``
  (``200`` + the whole body) is served from that body up to 256 MiB, and
  refused by name above it.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

import rete_graph as rete

native = pytest.mark.skipif(
    sys.platform == "emscripten", reason="the Pyodide wheel has no threads and no sockets"
)


def _graph_text(subjects: int) -> str:
    """Typed subjects with labels and links: every build phase has work."""
    lines = []
    for s in range(subjects):
        node = f"<http://example.org/n{s}>"
        lines.append(f"{node} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/C{s % 7}> .")
        lines.append(f'{node} <http://www.w3.org/2000/01/rdf-schema#label> "node {s} of the test graph" .')
        lines.append(f"{node} <http://example.org/next> <http://example.org/n{(s * 7919) % subjects}> .")
        lines.append(f'{node} <http://example.org/n> "{s}"^^<http://www.w3.org/2001/XMLSchema#integer> .')
    return "\n".join(lines) + "\n"


COUNT_Q = "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"


@native
def test_the_native_wheel_has_parallel_and_http():
    assert {"parallel", "http"} <= set(rete.__engine_features__), rete.__engine_features__


@native
def test_a_parallel_build_is_byte_identical_to_a_single_threaded_one(tmp_path):
    text = _graph_text(20_000)
    src = tmp_path / "g.nt"
    src.write_text(text)
    here = rete.build(text)
    # A fresh interpreter whose rayon pool has one thread: the serial schedule.
    script = (
        "import hashlib, sys, rete_graph as rete; "
        "print(hashlib.sha256(rete.build(open(sys.argv[1]).read())).hexdigest())"
    )
    env = dict(os.environ, RAYON_NUM_THREADS="1")
    one = subprocess.run(
        [sys.executable, "-c", script, str(src)], env=env, capture_output=True, text=True, check=True
    ).stdout.strip()
    assert hashlib.sha256(here).hexdigest() == one
    (row,) = rete.open(here).query(COUNT_Q)
    assert row["n"].to_python() == 80_000


def _ticks_during(call) -> tuple[int, float]:
    """Run ``call`` while a second thread counts; return (ticks, seconds)."""
    ticks = 0
    stop = threading.Event()

    def ticker():
        nonlocal ticks
        while not stop.is_set():
            ticks += 1
            time.sleep(0.001)

    t = threading.Thread(target=ticker, daemon=True)
    t.start()
    time.sleep(0.02)  # let it start ticking
    before = ticks
    t0 = time.perf_counter()
    call()
    took = time.perf_counter() - t0
    during = ticks - before
    stop.set()
    t.join()
    return during, took


@native
def test_engine_calls_release_the_gil():
    text = _graph_text(60_000)
    built = {}
    during, took = _ticks_during(lambda: built.setdefault("b", rete.build(text)))
    # Holding the GIL for the whole call would leave the ticker at ~0. The
    # bound is loose (a 1 ms sleep ticks far fewer than 1000 times a second on
    # a loaded runner); what matters is that it ran at all, many times.
    assert took > 0.05, f"the build was too quick ({took:.3f} s) to tell"
    assert during >= 10, f"{during} ticks in {took:.3f} s: the build held the GIL"

    g = rete.open(built["b"])
    q = (
        "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c . ?s <http://example.org/next> ?t . "
        "?t <http://www.w3.org/2000/01/rdf-schema#label> ?l } GROUP BY ?c"
    )
    during, took = _ticks_during(lambda: g.query(q))
    if took > 0.05:  # only meaningful when the query takes a while
        assert during >= 5, f"{during} ticks in {took:.3f} s: the query held the GIL"


class _IgnoreRange(BaseHTTPRequestHandler):
    """A plain static host: every GET is ``200`` with the whole body."""

    payload = b""
    gets = 0

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.payload)))
        self.end_headers()

    def do_GET(self):
        type(self).gets += 1
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.payload)))
        self.end_headers()
        self.wfile.write(self.payload)


class _HugeIgnoreRange(BaseHTTPRequestHandler):
    """Claims a 300 MiB file and ignores ``Range``; streams zeros until the
    client hangs up (it should refuse on the length, unread)."""

    size = 300 << 20

    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Content-Length", str(self.size))
        self.end_headers()

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(self.size))
        self.end_headers()
        chunk = b"\0" * 65536
        try:
            for _ in range(self.size // len(chunk)):
                self.wfile.write(chunk)
        except OSError:
            pass  # the client refused and closed: expected


@pytest.fixture
def serve_handler():
    servers = []

    def _serve(handler) -> str:
        srv = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        srv.daemon_threads = True
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        servers.append(srv)
        return f"http://127.0.0.1:{srv.server_address[1]}/graph.rete"

    yield _serve
    for srv in servers:
        srv.shutdown()
        srv.server_close()


@native
def test_a_host_that_ignores_range_is_served_from_its_whole_body(serve_handler):
    data = rete.build(_graph_text(2_000))
    handler = type("H", (_IgnoreRange,), {"payload": data, "gets": 0})
    g = rete.open(serve_handler(handler))
    local = rete.open(data)
    assert g.query(COUNT_Q)[0]["n"].to_python() == 8_000
    label_q = (
        "SELECT ?l WHERE { <http://example.org/n42> "
        "<http://www.w3.org/2000/01/rdf-schema#label> ?l }"
    )
    assert [r["l"].value for r in g.query(label_q)] == ["node 42 of the test graph"]
    assert list(g.iter_quads()) == list(local.iter_quads())
    assert handler.gets == 1, f"{handler.gets} GETs: the whole body was downloaded more than once"


@native
def test_a_whole_body_over_the_cap_is_refused_by_name(serve_handler):
    with pytest.raises(RuntimeError, match="ignored the Range header.*not kept in memory"):
        rete.open(serve_handler(_HugeIgnoreRange))
