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
- **Compiled, not interpreted.** Chicory translates the engine to JVM bytecode
  once per JVM and HotSpot JITs it from there — see
  [Execution mode](#execution-mode-compiled-by-default) for the numbers and the
  escape hatch.

This mirrors the design of the [browser client](../js) and the
[Python client](../python): `clients/` consumes `crates/`, one thin binding per
language.

Two Maven modules (Reactor under this directory):

| Module        | Artifact                     | What it gives you                                    |
| ------------- | ---------------------------- | ---------------------------------------------------- |
| `rete-client` | `io.github.caviri:rete-client` | the lightweight engine wrapper (Chicory only)      |
| `rete-rdf4j`  | `io.github.caviri:rete-rdf4j`  | a read-only [RDF4J](https://rdf4j.org/) `Sail`/`Repository` over a `.rete` |

Both read a `.rete` from three sources: an **in-memory** image, a **file on
disk**, or a **URL**. The last two are read *lazily* — only the byte ranges a
query touches are ever read, so file size stops being a limit (see [Lazy
(range-read) querying](#lazy-range-read-querying-any-size)). Named graphs are
exposed as RDF4J contexts. The binding is read-only.

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
use one per thread (loading is cheap after the first, see
[Execution mode](#execution-mode-compiled-by-default)) or guard calls with a lock.

### Lazy (range-read) querying: any size

The `byte[]` entry points above copy the whole image into **wasm32 linear
memory on every call**. That is a hard ceiling at roughly 700 MB and the JVM
heap has nothing to do with it — the wasm address space is what runs out, so
`-Xmx` does not move it. Open the file *by range* instead and the image never
enters linear memory at all:

```java
import java.nio.file.Path;
import java.net.URI;

// From disk — a FileChannel serves the ranges.
try (Rete rete = Rete.openFile(Path.of("/data/cordis.rete"))) {
    String json = rete.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10");
    System.out.println(rete.bytesRead() + " bytes read (≪ file size)");
}

// Over HTTP — Range requests serve the ranges. Same reader, same methods.
try (Rete rete = Rete.openRemote(URI.create("https://data.example.org/dataset.rete"))) {
    String json = rete.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10");
}
```

`info()`, `query(String)`, `graphs()`, `scanInGraph(g,s,p,o)` and
`scanQuads(s,p,o)` work on either. (`infoRemote()`, `queryRemote(String)`, …
are kept as aliases; they were never HTTP-specific, only HTTP-named.) A remote
resource must support HTTP range requests (206 / `Content-Range`); a file needs
nothing but read permission. Either way the open cost is paid once into a
resident handle whose block cache stays warm across queries — `close()` releases
it, and the file descriptor with it.

Measured in this repo's Java image (`--memory=12g`, `-Xmx8g`, one fresh JVM per
figure, peak RSS from the kernel's `VmHWM`, compiled engine):

| file | op | whole-image `byte[]` | `openFile(Path)` | |
| --- | --- | --- | --- | --- |
| `mirbase.rete` 39.2 MiB | `info()` | 14.3 s · 738 MB · 100% read | **1.3 s · 557 MB · 6.4% read** | 10.7× faster |
| | `query()` `LIMIT 100` | 14.4 s · 708 MB · 100% | **1.7 s · 544 MB · 8.9%** | 8.6× faster |
| `davidrumsey.rete` 71.3 MiB | `info()` | 30.4 s · 1004 MB · 100% | **1.4 s · 556 MB · 3.9%** | 22× faster |
| | `query()` `LIMIT 100` | 30.2 s · 935 MB · 100% | **1.7 s · 569 MB · 7.7%** | 17.7× faster |
| `cordis.rete` 763.9 MiB | `info()` | **fails** after 79.6 s · 4212 MB | **1.7 s · 582 MB · 6.0%** | — |
| | `query()` `LIMIT 100` | **fails** after 78.9 s · 4990 MB | **1.6 s · 587 MB · 5.6%** | — |

"fails" is `ReteException: decompression failed: out of memory`, raised inside
wasm with 8 GiB of JVM heap untouched.

About 545 MB of every figure in the right-hand column is the floor: a JVM plus
the one-off Chicory compile of the engine (see
[Execution mode](#execution-mode-compiled-by-default)). The graph itself costs
tens of megabytes, not gigabytes.

### Execution mode: compiled by default

Chicory can either **interpret** the wasm module or **compile** it to JVM
bytecode. This client compiles, via
[`com.dylibso.chicory:compiler`](https://chicory.dev) — the module is translated
once per JVM (and the parsed module cached with it), so every later `Rete.load()`
/ `openRemote()` in that JVM is nearly free, and HotSpot JITs the generated
classes from there.

Measured in this repo's own Java image (`--memory=12g`, `-Xmx8g`, one fresh JVM
per figure, peak RSS from the kernel's `VmHWM`), two calls per op:

| file | op | interpreted | compiled | |
| --- | --- | --- | --- | --- |
| `mirbase.rete` 39.2 MiB, 2.70 M quads | `info()` | 84.1 s / 79.8 s · 1332 MB | **12.8 s / 12.6 s · 839 MB** | 6.6× faster, 1.6× less RSS |
| | `query()` `LIMIT 100` | 82.0 s / 81.8 s · 1581 MB | **12.9 s / 13.0 s · 840 MB** | 6.3× faster, 1.9× less RSS |
| `davidrumsey.rete` 71.3 MiB, 5.00 M quads | `info()` | 162.7 s / 157.7 s · 2953 MB | **28.9 s / 28.6 s · 961 MB** | 5.6× faster, 3.1× less RSS |
| | `query()` `LIMIT 100` | 150.5 s / 150.9 s · 1776 MB | **28.8 s / 28.7 s · 1140 MB** | 5.2× faster, 1.6× less RSS |

(Interpreted figures are the best of two runs — the interpreter's wall time
varies by up to ±30% under load, while the compiled figures repeat within 2%.)

**The startup cost is real and is not hidden here.** Compiling the 1.56 MiB
engine takes about **0.8 s**, once per JVM: the first `Rete.load()` goes from
~0.5 s (parse only) to ~1.3 s (parse + compile), and resident memory after that
load goes from ~250 MB to ~555 MB. A process that loads the engine and does
nothing is therefore *worse off*. A process that touches a real file is ahead
after the first call.

Caching the parsed module alongside the compiled code also fixes a second cost:
on `main` every additional `Rete.load()` re-parsed the wasm at **224–292 ms**
each — which an RDF4J `Sail` pays *per connection*. It is now **1–7 ms**.

To force the interpreter, set the system property:

```sh
java -Drete.chicory.interpreter=true -jar your-app.jar
```

That is the right call in three cases:

- a **short-lived process** that opens one small file, runs one small query and
  exits — the one-off compile can cost more than it saves;
- a runtime that **forbids defining classes at execution time** — GraalVM
  `native-image` (closed-world) or Android (no JVM bytecode loader). For those,
  Chicory's build-time compiler (`chicory-compiler-maven-plugin`) is the real
  answer; this client does not use it yet;
- **excluding the dependency** to slim the classpath. Dropping
  `com.dylibso.chicory:compiler` still works — `Rete` logs one warning and falls
  back to the interpreter rather than failing to load.

Cost of shipping it: `+510 KiB` of dependency JARs (`compiler` plus ASM), no
change to the `rete-client` artifact itself, and no new licence obligations —
`compiler` is Apache-2.0, ASM is BSD-3-Clause.

### What this does *not* fix

Compiling makes the same work faster; it does not change how much work there is.
Local calls still copy the entire file into wasm linear memory **on every call**,
and every scan is materialized twice (a wasm `Vec`, then a JVM `List`), so the
local API is still bounded by `byte[]`/`Integer.MAX_VALUE` on one side and the
4 GiB `wasm32` linear memory on the other. `cordis.rete` (763.9 MiB) still dies
in `info()` with `decompression failed: out of memory` — inside linear memory,
with 8 GiB of heap unused — compiled exactly as it did interpreted, only sooner.
See [issue #115](https://github.com/caviri/rete/issues/115), which stays open.

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

### Ranged reads (the one import)

The module imports a single function the runtime supplies:

- `env.rete_host_read_range(offset: i64, len: i32, dest: i32) -> i32` — the host
  writes `len` bytes at `offset` of the resource into linear memory at `dest`
  and returns the count. Because the call is synchronous, the engine's sync
  range reads work without Asyncify.

**That import is the whole seam, and it is source-agnostic**: the Java side
satisfies it with an HTTP `Range` GET (`openRemote`) or a positional
`FileChannel.read` (`openFile`), and nothing in the wasm module can tell the
difference. This is the same shape the browser client uses — one
`XhrRangeReader` whose bottom transport is either `fetch` or `Blob.slice()`.

Ranged querying uses a resident handle: `rete_ranged_open(file_len) -> id`, then
`rete_handle_query(id, query)` / `rete_handle_scan_quads(id, …)` /
`rete_handle_scan_in_graph(id, …, g)` / `rete_handle_graphs(id)` /
`rete_handle_info(id)`, and `rete_handle_close(id)`. Opening once keeps the
block cache warm across a query's many scans. (`rete_remote_open` remains as an
alias of `rete_ranged_open`, for a host built against the older ABI.)
