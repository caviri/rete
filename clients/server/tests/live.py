"""A real server on a real port, inside the test process (uvicorn in a thread).

The task tests need the whole stack (the Docket worker, the HTTP transport,
the token gate) and a way to make the engine slow, which a subprocess cannot
give: the server's objects are reachable from here.
"""
from __future__ import annotations

import re
import socket
import threading
import time
from contextlib import contextmanager

import httpx
import rete_graph as rete
import uvicorn

from conftest import DATA, settings

BIG_ROWS = 500
BIG_NT = "".join(f'<http://ex.org/item{i}> <http://ex.org/value> "{i}"^^<http://www.w3.org/2001/XMLSchema#integer> .\n'
                 f'<http://ex.org/item{i}> <http://ex.org/label> "item {i}"@en .\n'
                 for i in range(BIG_ROWS))
(DATA / "numbers.rete").write_bytes(rete.build(BIG_NT, "nt"))
NUMBERS = "local/numbers.rete"
ALL_VALUES = "SELECT ?s ?v WHERE { ?s <http://ex.org/value> ?v } ORDER BY ?v"
ALL_TRIPLES = "CONSTRUCT { ?s <http://ex.org/value> ?v } WHERE { ?s <http://ex.org/value> ?v }"


class SlowGraph:
    """Wraps an engine graph; a query carrying ``# slow <seconds>`` sleeps first."""

    def __init__(self, graph):
        self._graph = graph

    def query_raw(self, query, *args, **kwargs):
        m = re.search(r"#\s*slow\s+([\d.]+)", query)
        if m:
            time.sleep(float(m.group(1)))
        return self._graph.query_raw(query, *args, **kwargs)

    def __getattr__(self, name):
        return getattr(self._graph, name)


def make_slow(dataset: str) -> None:
    """Both lanes of one dataset get a slow-able graph."""
    from rete_server import state

    source = state.service.resolve(dataset)[0]
    for lane in ("quick", "long"):
        handle = state.service.handle(source, lane)
        if not isinstance(handle.graph, SlowGraph):
            handle.graph = SlowGraph(handle.graph)


@contextmanager
def live(**overrides):
    """Yield the base URL of a running server built from ``settings(**overrides)``."""
    from rete_server.app import create_app

    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    server = uvicorn.Server(uvicorn.Config(create_app(settings(**overrides)), host="127.0.0.1",
                                           port=port, log_level="warning", lifespan="on"))
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{port}"
    deadline = time.time() + 30
    while True:
        try:
            if httpx.get(f"{base}/health", timeout=2).status_code == 200:
                break
        except httpx.TransportError:
            pass
        if time.time() > deadline:
            raise RuntimeError("test server did not start")
        time.sleep(0.1)
    try:
        yield base
    finally:
        server.should_exit = True
        thread.join(20)
