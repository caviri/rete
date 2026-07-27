# rete — Java clients

**Pure-Java** clients for the [rete](https://caviri.github.io/rete/)
cloud-native RDF graph file format.

The same Rust engine the native CLI and the browser use is compiled to a small,
**import-free WebAssembly module** and run *inside the JVM* by the
[Chicory](https://chicory.dev) runtime. That means:

- **No native library.** No JNI, no `.so`/`.dll`/`.dylib`, no per-platform
  artifacts to ship. Just a JAR that runs anywhere a JVM (21+) does.
- **No `wasm-bindgen` / JS glue.** The wasm boundary is a plain C ABI over
  linear memory (see [`ffi/src/lib.rs`](ffi/src/lib.rs)), so a JVM wasm runtime
  can call it directly.

This mirrors the design of the [browser client](../js) and the
[Python client](../python): `clients/` consumes `crates/`, one thin binding per
language.

Two Maven modules (Reactor under this directory):

| Module        | Artifact                     | What it gives you                                    |
| ------------- | ---------------------------- | ---------------------------------------------------- |
| `rete-client` | `io.github.caviri:rete-client` | the lightweight engine wrapper (Chicory only)      |
| `rete-rdf4j`  | `io.github.caviri:rete-rdf4j`  | a read-only [RDF4J](https://rdf4j.org/) `Sail`/`Repository` over a `.rete` |

Both work over an **in-memory** image or a **remote** `.rete` read lazily over
HTTP (only the byte ranges a query touches are fetched — the file is never fully
downloaded). Named graphs are exposed as RDF4J contexts. The binding is
read-only.

## `rete-client` usage

```java
import io.github.caviri.rete.Rete;
import java.nio.file.Files;
import java.nio.file.Path;

try (Rete rete = Rete.load()) {
    // Build a tiny graph from RDF text (or load a real file's bytes)...
    byte[] file = rete.build(
        "<http://example.org/book1> <http://purl.org/dc/terms/title> \"Rete\" .", "nt");
    // ...or: byte[] file = Files.readAllBytes(Path.of("dataset.rete"));

    System.out.println(rete.info(file));
    // {"schemaVersion":1,"quads":1,"terms":2,"pyramidLevels":..,"namedGraphs":..}

    String json = rete.query(file, "SELECT ?title WHERE { ?s <http://purl.org/dc/terms/title> ?title }");
    System.out.println(json);
    // {"kind":"select","vars":["title"],"rows":[{"title":"\"Rete\""}]}
}
```

The `query` result is the same JSON envelope the browser client returns:

| Query form           | Envelope                                                        |
| -------------------- | --------------------------------------------------------------- |
| `SELECT`             | `{"kind":"select","vars":[...],"rows":[{var:term,...},...]}`     |
| `ASK`                | `{"kind":"ask","boolean":true|false}`                           |
| `CONSTRUCT`/`DESCRIBE` | `{"kind":"construct","triples":[[s,p,o],...]}`                |

Engine errors (bad file, SPARQL parse/eval failure, invalid RDF) are raised as
`ReteException`, carrying the engine's own message.

A `Rete` instance owns a single wasm linear memory and is **not** thread-safe —
use one per thread (loading is cheap) or guard calls with a lock.

### Remote (lazy) querying

Query a `.rete` hosted over HTTP without downloading it — the engine issues
`Range` requests for only the bytes each query needs (the host fetch is done by
a synchronous Java `HttpClient` call, so no Asyncify is involved):

```java
import java.net.URI;

try (Rete rete = Rete.openRemote(URI.create("https://data.example.org/dataset.rete"))) {
    String json = rete.queryRemote("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10");
    System.out.println(rete.bytesFetched() + " bytes fetched (≪ file size)");
}
```

The resource must support HTTP range requests (206 / `Content-Range`). The
open cost (header + dictionary) is paid once into a resident handle whose block
cache stays warm across queries.

## `rete-rdf4j` usage

`ReteSail` makes a `.rete` a first-class, **read-only** [RDF4J](https://rdf4j.org/)
store, so RDF4J's own SPARQL engine, `SailRepository`, and Workbench can query it:

```java
import io.github.caviri.rete.rdf4j.ReteSail;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.sail.SailRepository;

byte[] file = Files.readAllBytes(Path.of("dataset.rete"));
Repository repo = new SailRepository(new ReteSail(file));
repo.init();
try (var conn = repo.getConnection();
     var result = conn.prepareTupleQuery("SELECT ?s ?p ?o WHERE { ?s ?p ?o }").evaluate()) {
    result.forEach(System.out::println);
}
```

RDF4J does the join/filter/ASK work, calling rete only for triple-pattern scans
(`getStatements`). rete terms are N-Triples syntax, so `NTriplesUtil` maps both
directions. Each connection loads its own wasm instance, so connections are
independent across threads.

**Named graphs** are exposed as RDF4J contexts: `getContextIDs()` lists them, a
plain pattern is the union of all graphs, `GRAPH <iri> { … }` restricts to one,
`GRAPH ?g { … }` binds `?g` to each named graph, and `getStatements(…, ctx)`
scopes to a context (`null` = the default graph). Each statement carries its
graph.

A **remote** Sail is `new ReteSail(uri)` — the same RDF4J API, backed by a
`.rete` read lazily over HTTP (each connection opens a resident handle whose
block cache is reused across the query's scans):

```java
Repository repo = new SailRepository(new ReteSail(URI.create("https://data.example.org/x.rete")));
repo.init();
```

The Sail is **read-only**.

### `ReteEngine` — the planner fast-path

Through the Sail, RDF4J's engine drives the query and calls rete once per triple
pattern (`getStatements`) — fully interoperable, but over a *remote* file that is
many small scans. `ReteEngine` instead hands the whole SPARQL string to rete's
own engine, which plans it and fetches the minimal set of byte ranges in one
shot, then returns the same RDF4J value types:

```java
try (ReteEngine engine = ReteEngine.openRemote(URI.create("https://data.example.org/x.rete"))) {
    List<BindingSet> rows = engine.select("SELECT ?p ?fn WHERE { ?p <http://ex/knows> ?f . ?f <http://ex/name> ?fn }");
    boolean any = engine.ask("ASK { ?s ?p ?o }");
    List<Statement> built = engine.construct("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }");
}
```

Results are differential-tested to match the Sail (RDF4J-engine) path exactly.
Use `ReteSail` for full `Repository`/Workbench interop; use `ReteEngine` when you
just want to run SPARQL with rete's planner (especially over HTTP).

## Build & test (Docker — no host toolchain)

Everything builds and tests in Docker, with the build context at the **repo
root** (the wasm crate path-deps `../../../crates`):

```sh
# Build the wasm engine + both JARs and run every test:
docker build -f clients/java/Dockerfile -t rete-java .

# Re-run the tests (offline; deps + wasm already in the image):
docker run --rm rete-java

# ...or via compose:
docker compose run --rm java
```

The build is two stages:

1. **`rust:1.92` → wasm.** Compiles the `ffi/` crate for
   `wasm32-unknown-unknown` into `rete_ffi.wasm` — whose only import is the
   host range-read function used on the remote path (no wasm-bindgen glue).
2. **`maven` + Temurin 21.** Drops that wasm into `rete-client`'s resources
   (where `Rete.load()` reads it) and runs `mvn verify` across the reactor.

The generated `rete_ffi.wasm` is **not committed** — it is produced fresh at
build time so it never lags the engine sources.

A **live** integration test (`LiveRemoteTest`) checks the client against a real
published dataset over HTTP; it is skipped by default (network) and enabled with
a flag:

```sh
docker run --rm rete-java \
  mvn -B -o -pl rete-client test -Dtest=LiveRemoteTest -Drete.live=true -DforkCount=0
```

It opens a 13.6 MB / 1.2 M-quad dataset over HTTP `Range` and queries it while
fetching ~17% of the file — the lazy path against production infrastructure.

CI runs the full (offline) suite on every change to `clients/java/**` or
`crates/rete-core/**` — see `.github/workflows/java-test.yml`.

## Layout

A Maven reactor with a shared wasm crate:

```
clients/java/
  pom.xml                  # parent (io.github.caviri:rete-parent:0.3.0)
  ffi/                     # Rust C-ABI crate → wasm32-unknown-unknown (no wasm-bindgen)
    Cargo.toml             #   excluded from the workspace; locks independently
    src/lib.rs             #   alloc/free + build/info/query/scan over linear memory
  rete-client/             # artifact rete-client — the Chicory engine wrapper (light)
    src/main/java/…/Rete.java, ReteException.java
    src/test/java/…/ReteTest.java          # self-contained end-to-end test
  rete-rdf4j/              # artifact rete-rdf4j — RDF4J Sail/Repository over a .rete
    src/main/java/…/rdf4j/ReteSail.java, ReteSailConnection.java
    src/test/java/…/rdf4j/ReteSailTest.java # SPARQL through the RDF4J engine
  Dockerfile               # multi-stage build+test (context = repo root)
```

## The wasm ABI

The Java side never touches raw pointers beyond a few helpers. The contract with
the wasm module (`ffi/src/lib.rs`):

- `rete_alloc(len) -> ptr` / `rete_free(ptr, len)` — host-driven allocation in
  the module's linear memory.
- `rete_version()`, `rete_build(text,fmt)`, `rete_info(bytes)`,
  `rete_query(bytes,query)` — each returns a pointer to a result buffer laid out
  as `[status: u32 LE][len: u32 LE][payload: len bytes]`. `status == 0` is
  success (`payload` is JSON, or raw `.rete` bytes for `build`); `status == 1`
  is an error (`payload` is a UTF-8 message).
- `rete_scan(bytes, s,p,o)` / `rete_scan_in_graph(…, g)` / `rete_scan_quads(…)`
  / `rete_graphs(bytes)` — triple/graph scans for the RDF4J `getStatements` and
  `getContextIDs` primitives (zero-length position = wildcard). Success payloads
  are length-framed binary blobs of N-Triples terms — no JSON escaping for a
  machine consumer.

The host reads `status`/`len`/`payload`, then frees every buffer it was handed.

### Remote (the one import)

The module imports a single function the runtime supplies:

- `env.rete_host_read_range(offset: i64, len: i32, dest: i32) -> i32` — the host
  writes `len` bytes at `offset` of the resource into linear memory at `dest`
  and returns the count. The Java side does an HTTP `Range` GET; because the
  call is synchronous, the engine's sync range reads work without Asyncify.

Remote querying uses a resident handle: `rete_remote_open(file_len) -> id`, then
`rete_handle_query(id, query)` / `rete_handle_scan_quads(id, …)` /
`rete_handle_scan_in_graph(id, …, g)` / `rete_handle_graphs(id)` /
`rete_handle_info(id)`, and `rete_handle_close(id)`. Opening once keeps the
block cache warm across a query's many scans.
