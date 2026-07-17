# JupyterLite — Python notebooks, no install

An experiment: a **full Jupyter notebook running the
[`rete-graph` Python client](python.md) entirely in your browser tab**. The
kernel is [Pyodide](https://pyodide.org) (CPython on WebAssembly) inside
[JupyterLite](https://jupyterlite.readthedocs.io); the package installs from
PyPI as a PyEmscripten wheel; remote `.rete` graphs are queried over HTTP
range requests from inside the tab. Nothing to install, no server anywhere in
the stack — not even for Python itself.

Two notebooks are bundled:

- **[Tour: query graphs →](jupyterlite/lab/index.html?path=rete-graph.ipynb)**
  — the client end to end: lazy remote graphs, SPARQL → pandas, embedded
  starter queries.
- **[Build: anatomy of a `.rete` →](jupyterlite/lab/index.html?path=build-a-rete.ipynb)**
  — author a small KG **with an ontology in rdflib**, build a `.rete` from
  it, and dissect every part of the file: the **dictionary** (terms stored
  once, integer-id triples), the **triple indexes**, the **pyramid** (built
  with the `types` algorithm, so the summary *is* the class structure), the
  **Dataset Card**, and the **embedded example queries** — ending with OWL 2
  QL reasoning inferring what was never asserted, and an exported file in the
  notebook's file browser.

The tour notebook, `rete-graph.ipynb`, walks through the whole client with
explanations between the cells:

1. `%pip install rete-graph pandas` — the PyEmscripten wheel resolves
   straight from PyPI;
2. **open the BOE legal graph lazily** (447k triples, 6.9 MB on Cloudflare
   R2) and watch `stats()` count how few bytes the open actually fetched;
3. **SPARQL → pandas** — `query_df()` renders results as real DataFrame
   tables, like the most-citing laws in the corpus;
4. **the dataset documents itself** — the starter queries embedded in the
   file's [Dataset Card](dataset-cards.md), listed as a table and then run
   as-is;
5. the **schema profile** as a table, and
6. **building a `.rete` in the browser** with the lazy `Builder`, card and
   embedded example included.

Run it top to bottom with `Shift+Enter`; the first cell takes a moment (it
boots Pyodide and downloads the wheel). Everything is editable — change the
queries, point `rete.open()` at any `.rete` URL whose host sends CORS +
`Range` (see [Hosting](hosting.md)).

Notes and limits, honestly: the kernel runs in a web worker, which is exactly
where the client's synchronous XHR range reads are allowed — so remote opens
work here, while `SERVICE` federation and threads stay disabled (see the
[Python API's browser notes](python.md)). The Pyodide runtime loads from a
CDN on first start, so the page needs network. Wheels are per-Pyodide-ABI;
this deployment pins a JupyterLite release whose Pyodide matches the wheels
published from this repo.

Source & issues: <https://github.com/caviri/rete> · © 2026 Carlos Vivar Ríos,
released under the
[Apache License 2.0](https://github.com/caviri/rete/blob/main/LICENSE).
Datasets keep their own licenses (BOE: © Agencia Estatal Boletín Oficial del
Estado, [reuse conditions](https://www.boe.es/datosabiertos/)).
