package io.github.caviri.rete;

import com.dylibso.chicory.compiler.InterpreterFallback;
import com.dylibso.chicory.compiler.MachineFactoryCompiler;
import com.dylibso.chicory.runtime.ExportFunction;
import com.dylibso.chicory.runtime.HostFunction;
import com.dylibso.chicory.runtime.ImportValues;
import com.dylibso.chicory.runtime.Instance;
import com.dylibso.chicory.runtime.Machine;
import com.dylibso.chicory.runtime.Memory;
import com.dylibso.chicory.runtime.WasmFunctionHandle;
import com.dylibso.chicory.wasm.Parser;
import com.dylibso.chicory.wasm.WasmModule;
import com.dylibso.chicory.wasm.types.FunctionType;
import com.dylibso.chicory.wasm.types.ValType;

import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.System.Logger;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Function;

/**
 * A pure-Java client for the <a href="https://caviri.github.io/rete/">rete</a>
 * cloud-native RDF graph file format.
 *
 * <p>The same Rust engine the native CLI and the browser use is compiled to a
 * small, import-free WebAssembly module and run <em>inside the JVM</em> by the
 * <a href="https://chicory.dev">Chicory</a> runtime — no {@code wasm-bindgen}
 * JS glue and no native {@code .so}/{@code .dll}. The whole client is "just a
 * JAR that runs anywhere a JVM does".
 *
 * <p>Load the engine once, then query {@code .rete} images given as byte
 * arrays:
 *
 * <pre>{@code
 * try (Rete rete = Rete.load()) {
 *     byte[] file = rete.build("<http://a> <http://p> <http://b> .", "nt");
 *     String json = rete.query(file, "SELECT * WHERE { ?s ?p ?o }");
 *     System.out.println(json);
 * }
 * }</pre>
 *
 * <p><b>Lazy (range-read) querying — the way to open anything large.</b> The
 * {@code byte[]} entry points above copy the whole image into wasm32 linear
 * memory on every call, which is a hard wall around 700&nbsp;MB regardless of
 * {@code -Xmx}: the address space, not the heap, runs out. {@link #openFile(Path)}
 * and {@link #openRemote(URI)} instead open the file <em>by range</em> — the
 * engine asks the host for the byte ranges a query actually touches and the
 * image never enters linear memory at all:
 *
 * <pre>{@code
 * try (Rete rete = Rete.openFile(Path.of("/data/cordis.rete"))) {   // 764 MB, or 52 GB
 *     String json = rete.query("SELECT * WHERE { ?s ?p ?o } LIMIT 10");
 *     System.out.println(rete.bytesRead() + " bytes read");
 * }
 * try (Rete rete = Rete.openRemote(URI.create("https://data.example.org/x.rete"))) {
 *     String json = rete.query("SELECT * WHERE { ?s ?p ?o } LIMIT 10");
 * }
 * }</pre>
 *
 * <p>Both are the <em>same</em> reader: the engine's range reads are satisfied
 * by one host function this client supplies, and only its bottom transport
 * differs — an HTTP {@code Range} request or a {@link FileChannel} positional
 * read. (Because a JVM host call is synchronous, no Asyncify is needed, unlike
 * the browser.) The no-argument {@link #info()}, {@link #query(String)},
 * {@link #graphs()}, {@code scanInGraph} and {@code scanQuads} methods work on
 * either; the {@code …Remote} spellings are kept as aliases.
 *
 * <p><b>Streaming a scan.</b> The list-returning {@link #scanQuads(String,
 * String, String)} makes the engine build the <em>whole</em> result inside wasm
 * before returning, which for an unconstrained {@code (null, null, null)} is the
 * same wall in a different place. {@link #scanCursor(String, String, String)}
 * returns a {@link QuadCursor} that pulls bounded batches instead, so
 * time-to-first-row and peak memory are set by the batch rather than by the
 * graph:
 *
 * <pre>{@code
 * try (Rete rete = Rete.openFile(Path.of("/data/cordis.rete"));
 *      QuadCursor rows = rete.scanCursor(null, null, null)) {
 *     while (rows.hasNext()) {
 *         String[] quad = rows.next();   // {s, p, o, graph}
 *     }
 * }
 * }</pre>
 *
 * <p><b>Thread-safety:</b> a {@code Rete} instance owns a single wasm linear
 * memory and is <em>not</em> thread-safe. Use one instance per thread (loading
 * is cheap after the first one — see below), or guard calls with your own lock.
 *
 * <p><b>Execution mode.</b> By default the engine is <em>compiled</em>: Chicory
 * translates the wasm module to JVM bytecode once per JVM, and the HotSpot JIT
 * takes it from there. That costs a one-off compile at the first {@link #load()}
 * / {@link #openRemote(URI)} (roughly a second, paid once and shared by every
 * later instance) and makes query work several times faster than Chicory's
 * interpreter. Set the system property {@code rete.chicory.interpreter=true} to
 * force the interpreter instead — appropriate for a very short-lived process
 * that does one tiny query, or for a runtime that forbids defining classes at
 * execution time (GraalVM {@code native-image}, Android). If the compiler is
 * absent or refuses to run, the client logs a warning and falls back to the
 * interpreter by itself; it never fails to load because of it.
 */
public final class Rete implements AutoCloseable {

    /** Classpath location of the bundled wasm engine (placed there at build time). */
    private static final String WASM_RESOURCE = "/io/github/caviri/rete/rete_ffi.wasm";

    /** Set to {@code true} to run the engine on Chicory's interpreter. */
    public static final String INTERPRETER_PROPERTY = "rete.chicory.interpreter";

    /**
     * Rows a {@link QuadCursor} pulls per wasm call (default
     * {@value #DEFAULT_SCAN_BATCH}). Read when a cursor is opened.
     *
     * <p>This is the <b>ceiling</b>: a cursor opens smaller and doubles up to it
     * (see {@link QuadCursor}), so the number below is what a long drain settles
     * at, not what the first row costs.
     *
     * <p>A straight trade: smaller batches cross the wasm boundary more often,
     * larger ones hold more rows at once. The default is the knee of the measured
     * curve, not a guess — draining all 2,701,457 quads of {@code mirbase.rete},
     * median of three fresh JVMs at each fixed size:
     *
     * <pre>
     *   batch      64   128   256   512  2048  8192      (rows per wasm call)
     *   drain    13.5  14.0  13.3  13.0  12.2  12.2  s
     * </pre>
     *
     * Throughput is flat at and above 2048 and degrades by 6–15% below it, where
     * the per-call cost stops being amortized. 2048 is therefore the smallest
     * ceiling that still reaches full drain throughput; the ramp takes care of
     * the latency end.
     */
    public static final String SCAN_BATCH_PROPERTY = "rete.scan.batch";

    private static final int DEFAULT_SCAN_BATCH = 2048;

    /** Result-buffer status codes, matching {@code ffi/src/lib.rs}. */
    private static final int STATUS_OK = 0;

    private static final Logger LOG = System.getLogger(Rete.class.getName());

    /**
     * The parsed engine module. Immutable and reusable: {@code Instance.Builder}
     * copies every mutable section out of it, so one parse serves every instance
     * (an RDF4J {@code Sail} opens one per connection).
     */
    private static WasmModule cachedModule;

    /**
     * The compiled machine factory for {@link #cachedModule}, or {@code null}
     * when running interpreted. {@code null} is a valid resolved value, hence
     * the separate {@link #machineFactoryResolved} flag.
     */
    private static Function<Instance, Machine> cachedMachineFactory;

    private static boolean machineFactoryResolved;

    private final Instance instance;
    private final Memory memory;
    private final RangeFetcher fetcher;
    private final ExportFunction alloc;
    private final ExportFunction free;
    private final ExportFunction versionFn;
    private final ExportFunction buildFn;
    private final ExportFunction infoFn;
    private final ExportFunction queryFn;
    private final ExportFunction scanFn;
    private final ExportFunction graphsFn;
    private final ExportFunction scanInGraphFn;
    private final ExportFunction scanQuadsFn;
    private final ExportFunction rangedOpenFn;
    private final ExportFunction handleCloseFn;
    private final ExportFunction handleInfoFn;
    private final ExportFunction handleQueryFn;
    private final ExportFunction handleGraphsFn;
    private final ExportFunction handleScanInGraphFn;
    private final ExportFunction handleScanQuadsFn;
    private final ExportFunction handleScanOpenFn;
    private final ExportFunction handleScanNextFn;
    private final ExportFunction handleScanCloseFn;
    private final ExportFunction openCursorsFn;

    /**
     * Ids of scan cursors whose Java {@link QuadCursor} was garbage-collected
     * without {@code close()}. The cleaner thread only enqueues here; the ids are
     * released on the owning thread by {@link #reapAbandonedCursors()}, because
     * this instance owns one wasm memory and calling into it from another thread
     * is not safe.
     */
    private final ConcurrentLinkedQueue<Integer> abandonedCursors = new ConcurrentLinkedQueue<>();

    /** Streaming accounting; see {@link #rowsStreamed()}. */
    private long rowsStreamed;

    private long batchCalls;

    /**
     * Resident ranged-handle id, or -1 when this instance has no open file
     * (i.e. it was created by {@link #load()} for {@code byte[]} querying).
     */
    private int handle = -1;

    private Rete(Instance instance, RangeFetcher fetcher) {
        this.instance = instance;
        this.fetcher = fetcher;
        this.memory = instance.memory();
        this.alloc = instance.export("rete_alloc");
        this.free = instance.export("rete_free");
        this.versionFn = instance.export("rete_version");
        this.buildFn = instance.export("rete_build");
        this.infoFn = instance.export("rete_info");
        this.queryFn = instance.export("rete_query");
        this.scanFn = instance.export("rete_scan");
        this.graphsFn = instance.export("rete_graphs");
        this.scanInGraphFn = instance.export("rete_scan_in_graph");
        this.scanQuadsFn = instance.export("rete_scan_quads");
        this.rangedOpenFn = instance.export("rete_ranged_open");
        this.handleCloseFn = instance.export("rete_handle_close");
        this.handleInfoFn = instance.export("rete_handle_info");
        this.handleQueryFn = instance.export("rete_handle_query");
        this.handleGraphsFn = instance.export("rete_handle_graphs");
        this.handleScanInGraphFn = instance.export("rete_handle_scan_in_graph");
        this.handleScanQuadsFn = instance.export("rete_handle_scan_quads");
        this.handleScanOpenFn = instance.export("rete_handle_scan_open");
        this.handleScanNextFn = instance.export("rete_handle_scan_next");
        this.handleScanCloseFn = instance.export("rete_handle_scan_close");
        this.openCursorsFn = instance.export("rete_open_cursors");
    }

    /**
     * Load the engine for querying <b>in-memory</b> {@code .rete} images. Cheap
     * enough to call per thread: the module is parsed and compiled once per JVM
     * and every later instance reuses that work — only the first call in a JVM
     * pays it.
     *
     * @throws IllegalStateException if the bundled wasm is missing (the JAR was
     *     built without the engine — see the build instructions in the README)
     */
    public static Rete load() {
        // The module imports a host range-read function (used only when a file
        // is open); a byte[]-only engine supplies a stub that is never called.
        return instantiate(RangeFetcher.NO_SOURCE);
    }

    /**
     * Open a {@code .rete} <b>on disk</b> for lazy, range-read querying: the file
     * is <em>not</em> read into memory, and each call reads only the byte ranges
     * it needs through a {@link FileChannel}.
     *
     * <p>This is the way to open a file larger than a few hundred megabytes. The
     * {@code byte[]} entry points ({@link #info(byte[])}, {@link #query(byte[],
     * String)}, …) copy the whole image into wasm32 linear memory <em>per
     * call</em>, so they fail on a large file with {@code out of memory} however
     * much JVM heap is available — the 4&nbsp;GiB wasm address space is what runs
     * out, and the heap is not involved. Nothing here enters linear memory but
     * the blocks a query touches.
     *
     * <p>The returned instance owns an open file descriptor; {@link #close()}
     * releases it, so use try-with-resources.
     *
     * @throws ReteException if the file cannot be opened or is not a {@code .rete}
     */
    public static Rete openFile(Path path) {
        FileRangeFetcher file = new FileRangeFetcher(path);
        try {
            return openRanged(file);
        } catch (RuntimeException e) {
            file.close();
            throw e;
        }
    }

    /**
     * Open a <b>remote</b> {@code .rete} over HTTP for lazy, range-read querying:
     * the file is not downloaded; each call fetches only the byte ranges it needs
     * via HTTP {@code Range} requests. The resource must support range requests
     * (HTTP 206 / {@code Content-Range}).
     *
     * @throws ReteException if the resource cannot be opened
     */
    public static Rete openRemote(URI url) {
        return openRanged(new HttpRangeFetcher(url));
    }

    /**
     * The one lazy-open path, shared by {@link #openFile} and {@link #openRemote}
     * — the transport is the only difference between them.
     */
    private static Rete openRanged(RangeFetcher source) {
        Rete rete = instantiate(source);
        rete.handle = readLe32(rete.readResult(rete.rangedOpenFn.apply(source.length())[0]), 0);
        return rete;
    }

    /** Instantiate the bundled wasm, supplying the host range-read import. */
    private static Rete instantiate(RangeFetcher fetcher) {
        WasmModule module = module();
        // The host side of env.rete_host_read_range(offset:i64, len:i32, dest:i32) -> i32:
        // fetch the range and copy it into the module's linear memory at `dest`.
        WasmFunctionHandle readRange =
                (Instance inst, long... args) -> {
                    long offset = args[0];
                    int len = (int) args[1];
                    int dest = (int) args[2];
                    byte[] bytes = fetcher.read(offset, len);
                    inst.memory().write(dest, bytes);
                    return new long[] {bytes.length};
                };
        HostFunction hostRead =
                new HostFunction(
                        "env",
                        "rete_host_read_range",
                        FunctionType.of(
                                List.of(ValType.I64, ValType.I32, ValType.I32),
                                List.of(ValType.I32)),
                        readRange);
        Instance.Builder builder =
                Instance.builder(module)
                        .withImportValues(ImportValues.builder().addFunction(hostRead).build());
        Function<Instance, Machine> machineFactory = machineFactory(module);
        if (machineFactory != null) {
            builder = builder.withMachineFactory(machineFactory);
        }
        return new Rete(builder.build(), fetcher);
    }

    /** Parse the bundled engine once per JVM; every instance reuses the module. */
    private static synchronized WasmModule module() {
        if (cachedModule == null) {
            try (InputStream in = Rete.class.getResourceAsStream(WASM_RESOURCE)) {
                if (in == null) {
                    throw new IllegalStateException(
                            "bundled rete wasm not found on classpath at " + WASM_RESOURCE
                                    + " — build the wasm engine first (see clients/java/README.md)");
                }
                cachedModule = Parser.parse(in);
            } catch (IOException e) {
                throw new UncheckedIOException("failed to read bundled rete wasm", e);
            }
        }
        return cachedModule;
    }

    /**
     * The single place the execution mode is chosen. Compiles the module to JVM
     * bytecode once per JVM (the expensive part; the resulting factory is cheap
     * to apply per instance), or returns {@code null} to run interpreted.
     */
    private static synchronized Function<Instance, Machine> machineFactory(WasmModule module) {
        if (!machineFactoryResolved) {
            machineFactoryResolved = true;
            cachedMachineFactory = compile(module);
        }
        return cachedMachineFactory;
    }

    /**
     * Whether the engine is running compiled. Only meaningful once an instance
     * has been created (that is when the mode is resolved). Package-private:
     * the mode is an implementation detail, exposed for the tests that assert
     * both paths are actually exercised.
     */
    static synchronized boolean compiled() {
        return machineFactoryResolved && cachedMachineFactory != null;
    }

    private static Function<Instance, Machine> compile(WasmModule module) {
        if (Boolean.getBoolean(INTERPRETER_PROPERTY)) {
            return null;
        }
        try {
            return MachineFactoryCompiler.builder(module)
                    // A wasm feature this compiler cannot emit degrades that one
                    // function to the interpreter instead of failing the load.
                    .withInterpreterFallback(InterpreterFallback.WARN)
                    .compile();
        } catch (LinkageError | RuntimeException e) {
            // The compiler was excluded from the classpath, or the runtime
            // forbids defining classes (GraalVM native-image, Android). The
            // interpreter still answers every query, just slower.
            LOG.log(
                    Logger.Level.WARNING,
                    "rete: falling back to the Chicory interpreter (queries will be several times"
                            + " slower); set -D" + INTERPRETER_PROPERTY + "=true to silence this",
                    e);
            return null;
        }
    }

    /** The engine version string (e.g. {@code "0.3.0"}). Needs no input. */
    public String version() {
        return new String(readResult(versionFn.apply()), StandardCharsets.UTF_8);
    }

    /**
     * Assemble a complete {@code .rete} file image from RDF text, entirely
     * in-process.
     *
     * @param rdf    the RDF source text
     * @param format {@code "nt"} (N-Triples), {@code "nq"} (N-Quads) or
     *               {@code "ttl"} (Turtle)
     * @return the raw {@code .rete} file bytes, ready for {@link #query} /
     *     {@link #info}. Sections are uncompressed (the wasm build has no zstd
     *     encoder); every reader accepts them, and {@code rete build} produces
     *     a compressed file.
     * @throws ReteException if the RDF cannot be parsed or is empty
     */
    public byte[] build(String rdf, String format) {
        byte[] textBytes = rdf.getBytes(StandardCharsets.UTF_8);
        byte[] fmtBytes = format.getBytes(StandardCharsets.UTF_8);
        int textPtr = writeInput(textBytes);
        int fmtPtr = writeInput(fmtBytes);
        try {
            long resultPtr = buildFn.apply(textPtr, textBytes.length, fmtPtr, fmtBytes.length)[0];
            return readResult(resultPtr);
        } finally {
            free.apply(textPtr, textBytes.length);
            free.apply(fmtPtr, fmtBytes.length);
        }
    }

    /**
     * Header summary of a {@code .rete} image as a JSON string:
     * {@code {"schemaVersion":1,"quads":N,"terms":N,"pyramidLevels":N,"namedGraphs":N}}.
     *
     * @throws ReteException if the bytes are not a valid {@code .rete} image
     */
    public String info(byte[] reteFile) {
        int ptr = writeInput(reteFile);
        try {
            long resultPtr = infoFn.apply(ptr, reteFile.length)[0];
            return new String(readResult(resultPtr), StandardCharsets.UTF_8);
        } finally {
            free.apply(ptr, reteFile.length);
        }
    }

    /**
     * Run a SPARQL query against a {@code .rete} image and return the result
     * envelope as JSON — the same shape the browser client produces:
     * <ul>
     *   <li>SELECT → {@code {"kind":"select","vars":[...],"rows":[{var:term,...},...]}}</li>
     *   <li>ASK → {@code {"kind":"ask","boolean":true|false}}</li>
     *   <li>CONSTRUCT/DESCRIBE → {@code {"kind":"construct","triples":[[s,p,o],...]}}</li>
     * </ul>
     *
     * @throws ReteException if the file is invalid or the query fails to
     *     parse/evaluate
     */
    public String query(byte[] reteFile, String sparql) {
        byte[] queryBytes = sparql.getBytes(StandardCharsets.UTF_8);
        int bytesPtr = writeInput(reteFile);
        int queryPtr = writeInput(queryBytes);
        try {
            long resultPtr =
                    queryFn.apply(bytesPtr, reteFile.length, queryPtr, queryBytes.length)[0];
            return new String(readResult(resultPtr), StandardCharsets.UTF_8);
        } finally {
            free.apply(bytesPtr, reteFile.length);
            free.apply(queryPtr, queryBytes.length);
        }
    }

    /**
     * Evaluate a single triple pattern — the primitive an RDF4J {@code Sail}
     * needs for {@code getStatements(s, p, o)}. A {@code null} position is a
     * wildcard; a bound position is an N-Triples term string (as produced by
     * RDF4J's {@code NTriplesUtil}). Returns each match as a
     * {@code String[]}{@code {subject, predicate, object}} of N-Triples term
     * strings.
     *
     * @throws ReteException if the file is invalid
     */
    public List<String[]> scan(byte[] reteFile, String subject, String predicate, String object) {
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        int bytesPtr = writeInput(reteFile);
        // Wildcards pass a null pointer / zero length — no allocation.
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        try {
            long resultPtr =
                    scanFn.apply(
                            bytesPtr, reteFile.length,
                            sPtr, sBytes == null ? 0 : sBytes.length,
                            pPtr, pBytes == null ? 0 : pBytes.length,
                            oPtr, oBytes == null ? 0 : oBytes.length)[0];
            return parseScan(readResult(resultPtr));
        } finally {
            free.apply(bytesPtr, reteFile.length);
            if (sBytes != null) {
                free.apply(sPtr, sBytes.length);
            }
            if (pBytes != null) {
                free.apply(pPtr, pBytes.length);
            }
            if (oBytes != null) {
                free.apply(oPtr, oBytes.length);
            }
        }
    }

    /**
     * The named graphs of the dataset (the default graph is unnamed and not
     * included) — the RDF4J {@code getContextIDs} primitive. Each identifier is
     * an N-Triples term string (e.g. {@code "<http://…>"}), the same encoding
     * rete uses for subjects/predicates/objects.
     */
    public List<String> graphs(byte[] reteFile) {
        int ptr = writeInput(reteFile);
        try {
            byte[] payload = readResult(graphsFn.apply(ptr, reteFile.length)[0]);
            int pos = 0;
            int count = readLe32(payload, pos);
            pos += 4;
            List<String> out = new ArrayList<>(count);
            for (int i = 0; i < count; i++) {
                int len = readLe32(payload, pos);
                pos += 4;
                out.add(new String(payload, pos, len, StandardCharsets.UTF_8));
                pos += len;
            }
            return out;
        } finally {
            free.apply(ptr, reteFile.length);
        }
    }

    /**
     * A triple-pattern {@link #scan} scoped to one graph: {@code graph == null}
     * is the default graph, a non-null {@code graph} is a named graph given as
     * an N-Triples term (e.g. {@code "<http://…>"}, as returned by
     * {@link #graphs}). Each match is a {@code String[]}{@code {subject,
     * predicate, object}}.
     */
    public List<String[]> scanInGraph(
            byte[] reteFile, String graph, String subject, String predicate, String object) {
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        byte[] gBytes = graph == null ? null : graph.getBytes(StandardCharsets.UTF_8);
        int bytesPtr = writeInput(reteFile);
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        int gPtr = gBytes == null ? 0 : writeInput(gBytes);
        try {
            long resultPtr =
                    scanInGraphFn.apply(
                            bytesPtr, reteFile.length,
                            sPtr, len(sBytes),
                            pPtr, len(pBytes),
                            oPtr, len(oBytes),
                            gPtr, len(gBytes))[0];
            return parseScan(readResult(resultPtr));
        } finally {
            free.apply(bytesPtr, reteFile.length);
            freeIf(sPtr, sBytes);
            freeIf(pPtr, pBytes);
            freeIf(oPtr, oBytes);
            freeIf(gPtr, gBytes);
        }
    }

    /**
     * A triple-pattern scan across the default graph <em>and</em> every named
     * graph. Each match is a {@code String[]}{@code {subject, predicate, object,
     * graph}} where {@code graph} is {@code null} for the default graph, or the
     * named graph as an N-Triples term string otherwise.
     */
    public List<String[]> scanQuads(byte[] reteFile, String subject, String predicate, String object) {
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        int bytesPtr = writeInput(reteFile);
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        try {
            long resultPtr =
                    scanQuadsFn.apply(
                            bytesPtr, reteFile.length,
                            sPtr, len(sBytes),
                            pPtr, len(pBytes),
                            oPtr, len(oBytes))[0];
            return parseQuads(readResult(resultPtr));
        } finally {
            free.apply(bytesPtr, reteFile.length);
            freeIf(sPtr, sBytes);
            freeIf(pPtr, pBytes);
            freeIf(oPtr, oBytes);
        }
    }

    private static int len(byte[] b) {
        return b == null ? 0 : b.length;
    }

    private void freeIf(int ptr, byte[] b) {
        if (b != null) {
            free.apply(ptr, b.length);
        }
    }

    /** Parse the quad framing {@code [count][ (len,bytes)×3 , (glen,gbytes) ]×count}. */
    private static List<String[]> parseQuads(byte[] payload) {
        int pos = 0;
        int count = readLe32(payload, pos);
        pos += 4;
        List<String[]> quads = new ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            String[] quad = new String[4];
            for (int j = 0; j < 3; j++) {
                int len = readLe32(payload, pos);
                pos += 4;
                quad[j] = new String(payload, pos, len, StandardCharsets.UTF_8);
                pos += len;
            }
            int glen = readLe32(payload, pos);
            pos += 4;
            // Zero-length graph field = the default graph → null.
            quad[3] = glen == 0 ? null : new String(payload, pos, glen, StandardCharsets.UTF_8);
            pos += glen;
            quads.add(quad);
        }
        return quads;
    }

    /** Parse the {@code [count][ (len,bytes)×3 ]×count} framing from {@code rete_scan}. */
    private static List<String[]> parseScan(byte[] payload) {
        int pos = 0;
        int count = readLe32(payload, pos);
        pos += 4;
        List<String[]> triples = new ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            String[] triple = new String[3];
            for (int j = 0; j < 3; j++) {
                int len = readLe32(payload, pos);
                pos += 4;
                triple[j] = new String(payload, pos, len, StandardCharsets.UTF_8);
                pos += len;
            }
            triples.add(triple);
        }
        return triples;
    }

    private static int readLe32(byte[] b, int i) {
        return (b[i] & 0xFF)
                | ((b[i + 1] & 0xFF) << 8)
                | ((b[i + 2] & 0xFF) << 16)
                | ((b[i + 3] & 0xFF) << 24);
    }

    // --- operations on an open file (lazy, range-read) ---------------------
    //
    // These work identically for a file opened with openFile(Path) or
    // openRemote(URI): they run against the resident handle, and the reader
    // underneath faults in only the ranges each call touches.

    /** Header summary of the open {@code .rete} (see {@link #info(byte[])}). */
    public String info() {
        checkOpen();
        return new String(readResult(handleInfoFn.apply(handle)[0]), StandardCharsets.UTF_8);
    }

    /** SPARQL over the open {@code .rete} (see {@link #query(byte[], String)}). */
    public String query(String sparql) {
        checkOpen();
        byte[] q = sparql.getBytes(StandardCharsets.UTF_8);
        int qPtr = writeInput(q);
        try {
            return new String(
                    readResult(handleQueryFn.apply(handle, qPtr, q.length)[0]),
                    StandardCharsets.UTF_8);
        } finally {
            free.apply(qPtr, q.length);
        }
    }

    /** Named graphs of the open {@code .rete} (see {@link #graphs(byte[])}). */
    public List<String> graphs() {
        checkOpen();
        byte[] payload = readResult(handleGraphsFn.apply(handle)[0]);
        int pos = 0;
        int count = readLe32(payload, pos);
        pos += 4;
        List<String> out = new ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            int len = readLe32(payload, pos);
            pos += 4;
            out.add(new String(payload, pos, len, StandardCharsets.UTF_8));
            pos += len;
        }
        return out;
    }

    /**
     * Graph-scoped triple scan over the open {@code .rete} (see
     * {@link #scanInGraph(byte[], String, String, String, String)}).
     */
    public List<String[]> scanInGraph(String graph, String subject, String predicate, String object) {
        checkOpen();
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        byte[] gBytes = graph == null ? null : graph.getBytes(StandardCharsets.UTF_8);
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        int gPtr = gBytes == null ? 0 : writeInput(gBytes);
        try {
            long resultPtr =
                    handleScanInGraphFn.apply(
                            handle,
                            sPtr, len(sBytes),
                            pPtr, len(pBytes),
                            oPtr, len(oBytes),
                            gPtr, len(gBytes))[0];
            return parseScan(readResult(resultPtr));
        } finally {
            freeIf(sPtr, sBytes);
            freeIf(pPtr, pBytes);
            freeIf(oPtr, oBytes);
            freeIf(gPtr, gBytes);
        }
    }

    /**
     * All-graphs quad scan over the open {@code .rete} (see
     * {@link #scanQuads(byte[], String, String, String)}).
     */
    public List<String[]> scanQuads(String subject, String predicate, String object) {
        checkOpen();
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        try {
            long resultPtr =
                    handleScanQuadsFn.apply(
                            handle,
                            sPtr, len(sBytes),
                            pPtr, len(pBytes),
                            oPtr, len(oBytes))[0];
            return parseQuads(readResult(resultPtr));
        } finally {
            freeIf(sPtr, sBytes);
            freeIf(pPtr, pBytes);
            freeIf(oPtr, oBytes);
        }
    }

    // --- streaming scans over an open file ---------------------------------

    /**
     * Open a <b>streaming</b> scan of a triple pattern across the default graph
     * <em>and</em> every named graph — the cursor form of
     * {@link #scanQuads(String, String, String)}.
     *
     * <p>Use this whenever the pattern is not narrow. {@code scanQuads} makes the
     * engine build the complete result inside wasm32 linear memory before the
     * first row is returned, so an unconstrained {@code (null, null, null)} over
     * a large graph exhausts the 4&nbsp;GiB address space however much JVM heap
     * there is. A cursor pulls {@value #DEFAULT_SCAN_BATCH} rows per wasm call
     * (override with the {@link #SCAN_BATCH_PROPERTY} system property), so
     * time-to-first-row and peak memory are bounded by the batch rather than by
     * the graph.
     *
     * <p>Each row is {@code {subject, predicate, object, graph}} in N-Triples
     * syntax, with a {@code null} graph for the default graph. Close it — see
     * {@link QuadCursor}.
     *
     * @throws IllegalStateException if no {@code .rete} is open on this engine
     */
    public QuadCursor scanCursor(String subject, String predicate, String object) {
        return openCursor(null, subject, predicate, object, true);
    }

    /**
     * A {@link #scanCursor} scoped to one graph: {@code graph == null} is the
     * default graph, a non-null {@code graph} is a named graph as an N-Triples
     * term (as returned by {@link #graphs()}). The cursor form of
     * {@link #scanInGraph(String, String, String, String)}; every row carries the
     * graph asked for.
     */
    public QuadCursor scanCursorInGraph(
            String graph, String subject, String predicate, String object) {
        return openCursor(graph, subject, predicate, object, false);
    }

    private QuadCursor openCursor(
            String graph, String subject, String predicate, String object, boolean allGraphs) {
        checkOpen();
        // Piggy-back the reap on any cursor open: a long-lived Sail that never
        // closes anything still cannot accumulate engine-side cursors.
        reapAbandonedCursors();
        byte[] sBytes = subject == null ? null : subject.getBytes(StandardCharsets.UTF_8);
        byte[] pBytes = predicate == null ? null : predicate.getBytes(StandardCharsets.UTF_8);
        byte[] oBytes = object == null ? null : object.getBytes(StandardCharsets.UTF_8);
        byte[] gBytes = graph == null ? null : graph.getBytes(StandardCharsets.UTF_8);
        int sPtr = sBytes == null ? 0 : writeInput(sBytes);
        int pPtr = pBytes == null ? 0 : writeInput(pBytes);
        int oPtr = oBytes == null ? 0 : writeInput(oBytes);
        int gPtr = gBytes == null ? 0 : writeInput(gBytes);
        try {
            long resultPtr =
                    handleScanOpenFn.apply(
                            handle,
                            sPtr, len(sBytes),
                            pPtr, len(pBytes),
                            oPtr, len(oBytes),
                            gPtr, len(gBytes),
                            allGraphs ? 1 : 0)[0];
            int cursorId = readLe32(readResult(resultPtr), 0);
            return new QuadCursor(this, cursorId, scanBatchSize(), abandonedCursors);
        } finally {
            freeIf(sPtr, sBytes);
            freeIf(pPtr, pBytes);
            freeIf(oPtr, oBytes);
            freeIf(gPtr, gBytes);
        }
    }

    /** One batch of a streaming scan: the rows, and whether the scan is finished. */
    record Batch(List<String[]> rows, boolean done) {}

    /** Pull one batch from an open cursor. Package-private: {@link QuadCursor} drives it. */
    Batch nextBatch(int cursorId, int maxRows) {
        batchCalls++;
        byte[] payload = readResult(handleScanNextFn.apply(cursorId, maxRows)[0]);
        int count = readLe32(payload, 0);
        boolean done = readLe32(payload, 4) != 0;
        int pos = 8;
        List<String[]> rows = new ArrayList<>(count);
        for (int i = 0; i < count; i++) {
            String[] quad = new String[4];
            for (int j = 0; j < 3; j++) {
                int len = readLe32(payload, pos);
                pos += 4;
                quad[j] = new String(payload, pos, len, StandardCharsets.UTF_8);
                pos += len;
            }
            int glen = readLe32(payload, pos);
            pos += 4;
            quad[3] = glen == 0 ? null : new String(payload, pos, glen, StandardCharsets.UTF_8);
            pos += glen;
            rows.add(quad);
        }
        rowsStreamed += count;
        return new Batch(rows, done);
    }

    /**
     * How many rows this engine has produced through streaming cursors, and how
     * many wasm calls it took. The companion to {@link #bytesRead()}: it is what
     * shows a scan was answered <em>incrementally</em>. A {@code LIMIT 1} over a
     * 26-million-quad graph that leaves this at a few thousand streamed; one that
     * leaves it at 26 million did not.
     */
    public long rowsStreamed() {
        return rowsStreamed;
    }

    /** Wasm calls made to pull batches — see {@link #rowsStreamed()}. */
    public long batchCalls() {
        return batchCalls;
    }

    /** Release one cursor. Idempotent in the engine; a no-op once the file is closed. */
    void closeCursor(int cursorId) {
        if (handle < 0) {
            return; // closing the handle already dropped every cursor on it
        }
        readResult(handleScanCloseFn.apply(cursorId)[0]);
    }

    /**
     * Release the cursors whose {@link QuadCursor} was collected without
     * {@code close()}. Called automatically whenever a cursor is opened and on
     * {@link #close()}; public so a caller that opens cursors rarely but holds
     * the engine for a long time can force it.
     *
     * @return how many were released
     */
    public int reapAbandonedCursors() {
        int n = 0;
        Integer id;
        while ((id = abandonedCursors.poll()) != null) {
            closeCursor(id);
            n++;
        }
        return n;
    }

    /**
     * How many scan cursors this engine currently holds open. Zero is the
     * invariant a long-lived {@code Sail} depends on; a number that climbs with
     * the query count is a leak.
     */
    public int openCursorCount() {
        return readLe32(readResult(openCursorsFn.apply()[0]), 0);
    }

    /** Rows pulled per wasm call by a {@link QuadCursor}; see {@link #SCAN_BATCH_PROPERTY}. */
    private static int scanBatchSize() {
        int n = Integer.getInteger(SCAN_BATCH_PROPERTY, DEFAULT_SCAN_BATCH);
        return n > 0 ? n : DEFAULT_SCAN_BATCH;
    }

    // The …Remote spellings, from before a local file could be opened this way.
    // Kept as aliases: they were never HTTP-specific, only HTTP-named.

    /** Alias of {@link #info()}. */
    public String infoRemote() {
        return info();
    }

    /** Alias of {@link #query(String)}. */
    public String queryRemote(String sparql) {
        return query(sparql);
    }

    /** Alias of {@link #graphs()}. */
    public List<String> graphsRemote() {
        return graphs();
    }

    /** Alias of {@link #scanInGraph(String, String, String, String)}. */
    public List<String[]> scanInGraphRemote(
            String graph, String subject, String predicate, String object) {
        return scanInGraph(graph, subject, predicate, object);
    }

    /** Alias of {@link #scanQuads(String, String, String)}. */
    public List<String[]> scanQuadsRemote(String subject, String predicate, String object) {
        return scanQuads(subject, predicate, object);
    }

    /**
     * Bytes read from the backing file so far — over HTTP for
     * {@link #openRemote}, from disk for {@link #openFile}. Zero for an engine
     * created by {@link #load()}, which has no backing file.
     */
    public long bytesRead() {
        return fetcher.bytesFetched();
    }

    /** Alias of {@link #bytesRead()}. */
    public long bytesFetched() {
        return bytesRead();
    }

    private void checkOpen() {
        if (handle < 0) {
            throw new IllegalStateException(
                    "no .rete is open on this engine — use Rete.openFile(path) or"
                            + " Rete.openRemote(uri)");
        }
    }

    /**
     * Releases the resident handle and the backing file descriptor (if any).
     * Chicory instances are plain heap objects reclaimed by the GC, so an engine
     * from {@link #load()} has nothing to release; {@code close()} still lets
     * callers use try-with-resources uniformly.
     */
    @Override
    public void close() {
        if (handle >= 0) {
            // Dropping the handle drops every scan cursor on it, so a caller who
            // abandoned one still leaks nothing past the file's own lifetime.
            handleCloseFn.apply(handle);
            handle = -1;
        }
        abandonedCursors.clear();
        fetcher.close();
    }

    // --- linear-memory plumbing -------------------------------------------

    /** Reserve module memory, copy {@code data} in, and return the pointer. */
    private int writeInput(byte[] data) {
        int ptr = ptr(alloc.apply(data.length));
        memory.write(ptr, data);
        return ptr;
    }

    /**
     * Read a {@code [status:u32][len:u32][payload]} result buffer at
     * {@code resultPtr}, free it, and return the payload — or throw the payload
     * as a {@link ReteException} when {@code status != 0}.
     */
    private byte[] readResult(long[] applyResult) {
        return readResult(applyResult[0]);
    }

    private byte[] readResult(long resultPtr) {
        int ptr = (int) (resultPtr & 0xFFFF_FFFFL);
        int status = memory.readInt(ptr);
        int len = memory.readInt(ptr + 4);
        byte[] payload = memory.readBytes(ptr + 8, len);
        free.apply(resultPtr, 8L + len);
        if (status != STATUS_OK) {
            throw new ReteException(new String(payload, StandardCharsets.UTF_8));
        }
        return payload;
    }

    /** Interpret an i32 export result as an unsigned pointer. */
    private static int ptr(long[] applyResult) {
        return (int) (applyResult[0] & 0xFFFF_FFFFL);
    }

    // --- range backend ----------------------------------------------------

    /**
     * Supplies byte ranges of the backing resource to the wasm engine — the one
     * seam between the engine and where the bytes live. The engine cannot tell
     * an implementation apart from any other, which is exactly why a local file
     * needed no new reader on the Rust side: it is the HTTP path with a
     * different {@link #read} underneath.
     */
    interface RangeFetcher {
        /** Read exactly {@code len} bytes at {@code offset}. */
        byte[] read(long offset, int len);

        /** Total size of the resource, in bytes. */
        default long length() {
            return 0L;
        }

        /** Total bytes read so far. */
        default long bytesFetched() {
            return 0L;
        }

        /** Release any OS resource held (a socket, a file descriptor). */
        default void close() {
            // nothing by default
        }

        /** An engine with no file open never reads ranges; a call here is a bug. */
        RangeFetcher NO_SOURCE =
                (offset, len) -> {
                    throw new IllegalStateException(
                            "range read attempted on an engine with no .rete open"
                                    + " (use Rete.openFile or Rete.openRemote)");
                };
    }

    /**
     * The local transport: one positional {@link FileChannel} read per range.
     *
     * <p>Batched like the HTTP one (the engine coalesces and block-aligns ranges
     * before they get here), but with no round trip to pay for — the only real
     * cost is the copy of what was asked for. {@code FileChannel.read(ByteBuffer,
     * long)} is positional, so it neither moves nor shares a file position and is
     * safe if the engine is ever driven from more than one thread.
     */
    private static final class FileRangeFetcher implements RangeFetcher {
        private final Path path;
        private final FileChannel channel;
        private final long total;
        private final AtomicLong read = new AtomicLong();

        FileRangeFetcher(Path path) {
            this.path = path;
            try {
                this.channel = FileChannel.open(path, StandardOpenOption.READ);
                this.total = channel.size();
            } catch (IOException e) {
                throw new ReteException("cannot open " + path + ": " + e);
            }
        }

        @Override
        public long length() {
            return total;
        }

        @Override
        public long bytesFetched() {
            return read.get();
        }

        @Override
        public byte[] read(long offset, int len) {
            ByteBuffer buf = ByteBuffer.allocate(len);
            long pos = offset;
            try {
                while (buf.hasRemaining()) {
                    int n = channel.read(buf, pos);
                    if (n < 0) {
                        throw new ReteException(
                                "short read at offset " + offset + " of " + path + ": wanted "
                                        + len + ", got " + buf.position());
                    }
                    pos += n;
                }
            } catch (IOException e) {
                throw new ReteException("range read failed on " + path + ": " + e);
            }
            read.addAndGet(len);
            return buf.array();
        }

        @Override
        public void close() {
            try {
                channel.close();
            } catch (IOException e) {
                throw new UncheckedIOException("closing " + path, e);
            }
        }
    }

    /** An HTTP {@code Range}-request fetcher; also counts the bytes it fetches. */
    private static final class HttpRangeFetcher implements RangeFetcher {
        private final URI url;
        private final HttpClient http;
        private final long total;
        private final AtomicLong fetched = new AtomicLong();

        HttpRangeFetcher(URI url) {
            this.url = url;
            this.http = HttpClient.newHttpClient();
            this.total = probeLength();
        }

        @Override
        public long length() {
            return total;
        }

        @Override
        public long bytesFetched() {
            return fetched.get();
        }

        @Override
        public byte[] read(long offset, int len) {
            byte[] body = range(offset, offset + (long) len - 1).body();
            fetched.addAndGet(body.length);
            return body;
        }

        /** Learn the total size from a 1-byte range read's {@code Content-Range}. */
        private long probeLength() {
            HttpResponse<byte[]> r = range(0, 0);
            String cr =
                    r.headers()
                            .firstValue("Content-Range")
                            .orElseThrow(
                                    () ->
                                            new ReteException(
                                                    "remote resource returned no Content-Range"
                                                        + " (no range support?): "
                                                            + url));
            int slash = cr.lastIndexOf('/');
            try {
                return Long.parseLong(cr.substring(slash + 1).trim());
            } catch (NumberFormatException e) {
                throw new ReteException("could not parse total size from Content-Range: " + cr);
            }
        }

        private HttpResponse<byte[]> range(long start, long end) {
            HttpRequest req =
                    HttpRequest.newBuilder(url)
                            .header("Range", "bytes=" + start + "-" + end)
                            .GET()
                            .build();
            try {
                HttpResponse<byte[]> r = http.send(req, HttpResponse.BodyHandlers.ofByteArray());
                if (r.statusCode() != 206) {
                    throw new ReteException(
                            "range request expected HTTP 206 but got "
                                    + r.statusCode()
                                    + " for "
                                    + url);
                }
                return r;
            } catch (IOException e) {
                throw new ReteException("range fetch failed for " + url + ": " + e);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new ReteException("range fetch interrupted for " + url);
            }
        }
    }
}
