//! Thin **C-ABI** wrapper around `rete-core`, compiled to
//! `wasm32-unknown-unknown` with **no `wasm-bindgen`**. The point is a wasm
//! module with (essentially) no imports, so a pure-Java wasm runtime (Chicory)
//! can instantiate it and call its exports directly — no JS glue, no native
//! `.so`/`.dll`, "just a JAR that runs anywhere". This mirrors the browser
//! `rete-wasm` crate, but the boundary is plain pointers + linear memory
//! instead of the `wasm-bindgen` ABI.
//!
//! ## Calling convention
//!
//! The host (Java) owns the choreography:
//!
//! 1. `rete_alloc(len) -> ptr` reserves `len` bytes of module memory; the host
//!    writes its input (a `.rete` image, a query string, …) at `ptr`.
//! 2. A `rete_*` entry point runs and returns a pointer to a **result buffer**
//!    laid out as `[status: u32 LE][len: u32 LE][payload: len bytes]`:
//!    - `status == 0` → success, `payload` is the result (JSON text, or raw
//!      `.rete` bytes for [`rete_build`]).
//!    - `status == 1` → error, `payload` is a UTF-8 message.
//! 3. The host reads `status`/`len`, copies `payload`, then returns every
//!    buffer it was handed (its own `rete_alloc` inputs and every result
//!    buffer) via `rete_free(ptr, total_len)`, where `total_len` is the exact
//!    byte length of that buffer (`8 + len` for a result buffer).
//!
//! Every allocation is an exact-capacity boxed slice, so `rete_free`'s `len`
//! is an exact inverse — no hidden capacity to leak.

use rete_core::{
    eval_query, results_envelope_json, BlockCacheReader, Rete, RangeReader, DEFAULT_BLOCK,
};

const STATUS_OK: u32 = 0;
const STATUS_ERR: u32 = 1;

/// A `custom` getrandom backend for `wasm32-unknown-unknown` running in a bare
/// JVM host (Chicory), which has no `crypto.getRandomValues` and no OS entropy.
/// rete-core reaches getrandom only for the SPARQL `RAND`/`UUID`/`STRUUID`/
/// `BNODE` built-ins, so those get **non-cryptographic** bytes from a
/// process-global xorshift* — deterministic per process, varying across calls.
/// A caller that needs real entropy for those built-ins should query through a
/// host that provides it. This is never used for anything security-relevant in
/// the read/query path.
fn insecure_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    use core::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    // Advance the shared state once per call so successive calls differ.
    let mut x = STATE.fetch_add(0x2545_F491_4F6C_DD1D, Ordering::Relaxed) | 1;
    for chunk in buf.chunks_mut(8) {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let bytes = x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        for (dst, src) in chunk.iter_mut().zip(bytes.iter()) {
            *dst = *src;
        }
    }
    Ok(())
}
getrandom::register_custom_getrandom!(insecure_getrandom);

/// Reserve `len` bytes of module-owned memory and return a pointer to it. The
/// host fills this with input bytes before calling an entry point, and frees it
/// with [`rete_free`] afterwards.
#[no_mangle]
pub extern "C" fn rete_alloc(len: u32) -> *mut u8 {
    let buf = vec![0u8; len as usize].into_boxed_slice();
    let ptr = buf.as_ptr() as *mut u8;
    std::mem::forget(buf);
    ptr
}

/// Free a buffer previously returned by [`rete_alloc`] or any `rete_*` result.
/// `len` must be the buffer's exact byte length (`8 + payload_len` for a
/// result buffer). A null pointer or zero length is a no-op.
///
/// # Safety
/// `ptr`/`len` must describe a buffer this module handed out and has not
/// already freed.
#[no_mangle]
pub unsafe extern "C" fn rete_free(ptr: *mut u8, len: u32) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(ptr, len as usize);
    drop(Box::from_raw(slice as *mut [u8]));
}

/// Pack `payload` into a freshly allocated `[status][len][payload]` buffer and
/// return a pointer to it (ownership passes to the host, which frees it).
fn pack(status: u32, payload: &[u8]) -> *mut u8 {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&status.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let boxed = out.into_boxed_slice();
    let ptr = boxed.as_ptr() as *mut u8;
    std::mem::forget(boxed);
    ptr
}

/// Borrow `len` bytes at `ptr` as a slice (empty for null/zero).
///
/// # Safety
/// `ptr`/`len` must describe readable module memory that outlives the borrow.
unsafe fn borrow<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len as usize)
    }
}

/// The `rete-core` version this module was built against, as a plain string
/// payload. Needs no input — the host's smoke test that the module loads and
/// calls end-to-end.
#[no_mangle]
pub extern "C" fn rete_version() -> *mut u8 {
    pack(STATUS_OK, env!("CARGO_PKG_VERSION").as_bytes())
}

// --- ranged (lazy) support ---------------------------------------------------
//
// A `.rete` is queried **without loading the whole file**: the engine's range
// reads are satisfied by a single host import the runtime supplies, so the file
// image never enters wasm32 linear memory. Unlike the browser (which needs
// Asyncify to fetch), a JVM host function is synchronous — it can do a blocking
// HTTP Range GET *or* a blocking `FileChannel.read` and return — so the sync
// engine "just works" over either.
//
// Nothing below knows or cares where the bytes come from: `rete_host_read_range`
// is the whole seam, and the host decides whether it is HTTP or a local file.
// That is why the local-file path needed no new reader here — exactly as the
// browser's local path is `XhrRangeReader` with a `Blob` under it (PR #200).

#[link(wasm_import_module = "env")]
extern "C" {
    /// The host reads `len` bytes at `offset` of the backing resource and writes
    /// them to `dest`, returning the number of bytes written (`len` on success;
    /// anything else is treated as a failed read).
    fn rete_host_read_range(offset: u64, len: u32, dest: *mut u8) -> u32;
}

/// A [`RangeReader`] whose reads are delegated to the host via
/// [`rete_host_read_range`]. Holds only the total resource length (learned by
/// the host before the engine opens).
struct HostRangeReader {
    total: u64,
}

impl RangeReader for HostRangeReader {
    fn len(&self) -> u64 {
        self.total
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len > u32::MAX as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "range read exceeds u32",
            ));
        }
        let mut buf = vec![0u8; len as usize];
        let got = unsafe { rete_host_read_range(offset, len as u32, buf.as_mut_ptr()) };
        if got as u64 != len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "host range read returned fewer bytes than requested",
            ));
        }
        Ok(buf)
    }
}

/// Block size for the read-through cache, scaled to file size (mirrors the
/// browser build): bigger files get bigger aligned block fetches.
fn auto_block(len: u64) -> u64 {
    const MB: u64 = 1 << 20;
    let mult: u64 = if len > 100 * MB {
        8
    } else if len > 10 * MB {
        4
    } else {
        2
    };
    mult * DEFAULT_BLOCK
}

/// Open a `.rete` of total size `file_len` lazily over the host range backend:
/// a block-caching reader that faults in only the ranges a query touches. The
/// backend may be HTTP or a local file — this code cannot tell.
fn open_ranged(file_len: u64) -> Result<Rete, String> {
    let cached = std::sync::Arc::new(BlockCacheReader::new(
        HostRangeReader { total: file_len },
        auto_block(file_len),
    ));
    Rete::open_ranged_lazy(cached).map_err(|e| e.to_string())
}

/// Turn a partial lazy fetch into an error: a ranged op must never return a
/// result computed over silently incomplete data.
fn guard_complete(rete: &Rete) -> Result<(), String> {
    if rete.index_incomplete() {
        Err("a range fetch failed mid-query; refusing to return incomplete results".to_string())
    } else {
        Ok(())
    }
}

/// Assemble a complete `.rete` file image from RDF text, entirely in-module.
///
/// `format` is `"nt"` (N-Triples), `"nq"` (N-Quads) or `"ttl"` (Turtle). The
/// success payload is the raw `.rete` **bytes** (not JSON) — hand them straight
/// back to [`rete_info`] / [`rete_query`]. As in the browser build there is no
/// zstd *encoder* here, so sections are written uncompressed (codec `NONE`);
/// every reader accepts them, and `rete build` produces a compressed file.
///
/// # Safety
/// The four pointer/length pairs must describe readable module memory.
#[no_mangle]
pub unsafe extern "C" fn rete_build(
    text_ptr: *const u8,
    text_len: u32,
    fmt_ptr: *const u8,
    fmt_len: u32,
) -> *mut u8 {
    let text = match std::str::from_utf8(borrow(text_ptr, text_len)) {
        Ok(t) => t,
        Err(e) => return pack(STATUS_ERR, format!("input text is not utf-8: {e}").as_bytes()),
    };
    let format = match std::str::from_utf8(borrow(fmt_ptr, fmt_len)) {
        Ok(f) => f,
        Err(e) => return pack(STATUS_ERR, format!("format is not utf-8: {e}").as_bytes()),
    };
    let quads = match rete_core::ingest::parse_statements(text, format) {
        Ok(q) => q,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    if quads.is_empty() {
        return pack(
            STATUS_ERR,
            b"no statements parsed (empty input or only comments)",
        );
    }
    let (bytes, _stats) = rete_core::ingest::assemble_dataset(quads, &[]);
    pack(STATUS_OK, &bytes)
}

/// Header summary of a `.rete` image as JSON:
/// `{ "schemaVersion", "quads", "terms", "pyramidLevels", "namedGraphs" }`.
///
/// # Safety
/// `bytes_ptr`/`bytes_len` must describe a readable `.rete` image in memory.
#[no_mangle]
pub unsafe extern "C" fn rete_info(bytes_ptr: *const u8, bytes_len: u32) -> *mut u8 {
    let bytes = borrow(bytes_ptr, bytes_len);
    match Rete::open(bytes) {
        Ok(rete) => {
            let h = rete.header();
            let json = format!(
                r#"{{"schemaVersion":1,"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
                h.quad_count,
                h.term_count,
                h.pyramid_levels,
                rete.graph_names().len()
            );
            pack(STATUS_OK, json.as_bytes())
        }
        Err(e) => pack(STATUS_ERR, e.to_string().as_bytes()),
    }
}

/// Evaluate a single **triple pattern** — the primitive an RDF4J `Sail` needs
/// for `getStatements(s, p, o)`. Each of the three positions is a pointer/length
/// pair; a **zero-length** position is a wildcard, a bound position is an
/// N-Triples term string (`<iri>`, `"lit"`, `"lit"@en`, `"lit"^^<dt>`, `_:b`).
/// The success payload is a JSON array of `[subject, predicate, object]`
/// triples. Because a `Sail` consumes this programmatically (not as display
/// text), the success payload is a compact **length-framed binary** blob rather
/// than JSON — no escaping, trivial to parse:
///
/// ```text
/// [count: u32 LE] then count × ( 3 × ( [termLen: u32 LE][term: UTF-8 bytes] ) )
/// ```
///
/// Each term is an N-Triples term string (the same syntax the host passes in),
/// so a JVM host maps both directions with one RDF term parser.
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[no_mangle]
pub unsafe extern "C" fn rete_scan(
    bytes_ptr: *const u8,
    bytes_len: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
) -> *mut u8 {
    let bytes = borrow(bytes_ptr, bytes_len);
    // A zero-length position is a wildcard (`None`); a bound one must be UTF-8.
    let term = |ptr, len| -> Result<Option<&str>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(Some)
        }
    };
    let (s, p, o) = match (term(s_ptr, s_len), term(p_ptr, p_len), term(o_ptr, o_len)) {
        (Ok(s), Ok(p), Ok(o)) => (s, p, o),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    let rete = match Rete::open(bytes) {
        Ok(r) => r,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    pack(STATUS_OK, &frame_triples(&rete.query(s, p, o)))
}

/// The named-graph IRIs of a dataset (the default graph is unnamed and not
/// listed) — the RDF4J `getContextIDs` primitive. Success payload is the
/// length-framed list `[count: u32 LE] then count × ([len: u32 LE][iri: UTF-8])`.
///
/// # Safety
/// `bytes_ptr`/`bytes_len` must describe a readable `.rete` image.
#[no_mangle]
pub unsafe extern "C" fn rete_graphs(bytes_ptr: *const u8, bytes_len: u32) -> *mut u8 {
    let rete = match Rete::open(borrow(bytes_ptr, bytes_len)) {
        Ok(r) => r,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    let names = rete.graph_names();
    let mut out = Vec::new();
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for iri in names {
        out.extend_from_slice(&(iri.len() as u32).to_le_bytes());
        out.extend_from_slice(iri.as_bytes());
    }
    pack(STATUS_OK, &out)
}

/// Serialize `rows` as the triple framing `[count][ (len,bytes)×3 ]×count`.
fn frame_triples(rows: &[(String, String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    for (rs, rp, ro) in rows {
        for term in [rs, rp, ro] {
            out.extend_from_slice(&(term.len() as u32).to_le_bytes());
            out.extend_from_slice(term.as_bytes());
        }
    }
    out
}

/// Like [`rete_scan`], but **scoped to one graph**: a zero-length `g` is the
/// default graph, a bound `g` is a named-graph IRI. Success payload is the same
/// triple framing as [`rete_scan`].
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn rete_scan_in_graph(
    bytes_ptr: *const u8,
    bytes_len: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
    g_ptr: *const u8,
    g_len: u32,
) -> *mut u8 {
    let term = |ptr, len| -> Result<Option<&str>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(Some)
        }
    };
    let (s, p, o, g) = match (
        term(s_ptr, s_len),
        term(p_ptr, p_len),
        term(o_ptr, o_len),
        term(g_ptr, g_len),
    ) {
        (Ok(s), Ok(p), Ok(o), Ok(g)) => (s, p, o, g),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    let rete = match Rete::open(borrow(bytes_ptr, bytes_len)) {
        Ok(r) => r,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    pack(STATUS_OK, &frame_triples(&rete.query_in_graph(g, s, p, o)))
}

/// Match a triple pattern across the default graph **and every named graph**,
/// tagging each with its graph — the quad-level scan behind an unconstrained
/// `getStatements`. Framing extends [`rete_scan`]'s with a fourth field per row:
///
/// ```text
/// [count: u32 LE] then count × ( 3 × [termLen][term] , [graphLen: u32 LE][graph: UTF-8] )
/// ```
///
/// A **zero-length** graph field means the default graph (a graph IRI is never
/// empty), so the host maps it to the null context.
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[no_mangle]
pub unsafe extern "C" fn rete_scan_quads(
    bytes_ptr: *const u8,
    bytes_len: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
) -> *mut u8 {
    let term = |ptr, len| -> Result<Option<&str>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(Some)
        }
    };
    let (s, p, o) = match (term(s_ptr, s_len), term(p_ptr, p_len), term(o_ptr, o_len)) {
        (Ok(s), Ok(p), Ok(o)) => (s, p, o),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    let rete = match Rete::open(borrow(bytes_ptr, bytes_len)) {
        Ok(r) => r,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    let quads = rete.query_quads(s, p, o);
    let mut out = Vec::new();
    out.extend_from_slice(&(quads.len() as u32).to_le_bytes());
    for ((rs, rp, ro), graph) in &quads {
        for term in [rs, rp, ro] {
            out.extend_from_slice(&(term.len() as u32).to_le_bytes());
            out.extend_from_slice(term.as_bytes());
        }
        let g = graph.as_deref().unwrap_or("");
        out.extend_from_slice(&(g.len() as u32).to_le_bytes());
        out.extend_from_slice(g.as_bytes());
    }
    pack(STATUS_OK, &out)
}

/// Run any SPARQL query form against a `.rete` image and return the playground
/// result envelope as JSON — the same shape the browser `query(.., "json")`
/// returns (SELECT → `{"kind":"select","head":…,"rows":…}`, ASK →
/// `{"kind":"ask","boolean":…}`, CONSTRUCT/DESCRIBE → `{"kind":"construct",
/// "triples":…}`), via the shared, host-tested `results_envelope_json`.
///
/// # Safety
/// Both pointer/length pairs must describe readable module memory; the first a
/// `.rete` image, the second a UTF-8 query.
#[no_mangle]
pub unsafe extern "C" fn rete_query(
    bytes_ptr: *const u8,
    bytes_len: u32,
    query_ptr: *const u8,
    query_len: u32,
) -> *mut u8 {
    let bytes = borrow(bytes_ptr, bytes_len);
    let query = match std::str::from_utf8(borrow(query_ptr, query_len)) {
        Ok(q) => q,
        Err(e) => return pack(STATUS_ERR, format!("query is not utf-8: {e}").as_bytes()),
    };
    let rete = match Rete::open(bytes) {
        Ok(r) => r,
        Err(e) => return pack(STATUS_ERR, e.to_string().as_bytes()),
    };
    match eval_query(&rete, query) {
        Ok(out) => pack(STATUS_OK, results_envelope_json(&out, "").as_bytes()),
        Err(e) => pack(STATUS_ERR, e.to_string().as_bytes()),
    }
}

// --- resident ranged handles -------------------------------------------------
//
// A `.rete` is opened once (`rete_ranged_open`) into a resident handle whose
// block cache stays warm across calls, then queried by id. This is what a
// consumer that issues many scans — an RDF4J `Sail` evaluating a SPARQL query —
// needs: re-opening per scan would re-read the header/dictionary every time.
// The engine is single-threaded, so a thread-local registry is sufficient.

thread_local! {
    static HANDLES: std::cell::RefCell<std::collections::HashMap<u32, Rete>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static NEXT_HANDLE: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

/// Open a `.rete` of total size `file_len` into a resident handle, reading it
/// lazily through [`rete_host_read_range`]. The success payload is the 4-byte LE
/// handle id; close it with [`rete_handle_close`].
///
/// **Source-agnostic**: the host decides what backs the ranges (an HTTP `Range`
/// GET, a `FileChannel`, a memory-mapped buffer, …). Nothing in this module
/// distinguishes them.
#[no_mangle]
pub extern "C" fn rete_ranged_open(file_len: u64) -> *mut u8 {
    match open_ranged(file_len) {
        Ok(rete) => {
            let id = NEXT_HANDLE.with(|n| {
                let id = n.get();
                n.set(id.wrapping_add(1).max(1));
                id
            });
            HANDLES.with(|h| h.borrow_mut().insert(id, rete));
            pack(STATUS_OK, &id.to_le_bytes())
        }
        Err(e) => pack(STATUS_ERR, e.as_bytes()),
    }
}

/// Former name of [`rete_ranged_open`], kept so a host built against the old
/// ABI still links. Identical behaviour.
#[no_mangle]
pub extern "C" fn rete_remote_open(file_len: u64) -> *mut u8 {
    rete_ranged_open(file_len)
}

/// Drop a resident ranged handle (idempotent). Every scan cursor still open on
/// that handle goes with it — a host that forgot to close one cannot leave a
/// dangling cursor behind, and a `Rete.close()` is always a full release.
#[no_mangle]
pub extern "C" fn rete_handle_close(id: u32) -> *mut u8 {
    CURSORS.with(|c| c.borrow_mut().retain(|_, cur| cur.handle != id));
    HANDLES.with(|h| h.borrow_mut().remove(&id));
    pack(STATUS_OK, &[])
}

/// Run `f` against the open `Rete` for `id`, or return an error buffer.
fn with_handle<F: FnOnce(&Rete) -> *mut u8>(id: u32, f: F) -> *mut u8 {
    HANDLES.with(|h| match h.borrow().get(&id) {
        Some(rete) => f(rete),
        None => pack(STATUS_ERR, b"invalid or closed ranged handle"),
    })
}

/// Header summary of a ranged handle (see [`rete_info`]).
#[no_mangle]
pub extern "C" fn rete_handle_info(id: u32) -> *mut u8 {
    with_handle(id, |rete| {
        let h = rete.header();
        let json = format!(
            r#"{{"schemaVersion":1,"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
            h.quad_count,
            h.term_count,
            h.pyramid_levels,
            rete.graph_names().len()
        );
        pack(STATUS_OK, json.as_bytes())
    })
}

/// SPARQL over a ranged handle (see [`rete_query`]); refuses incomplete results.
///
/// # Safety
/// The query pointer/length must describe readable module memory.
#[no_mangle]
pub unsafe extern "C" fn rete_handle_query(id: u32, q_ptr: *const u8, q_len: u32) -> *mut u8 {
    let query = match std::str::from_utf8(borrow(q_ptr, q_len)) {
        Ok(q) => q,
        Err(e) => return pack(STATUS_ERR, format!("query is not utf-8: {e}").as_bytes()),
    };
    with_handle(id, |rete| match eval_query(rete, query) {
        Ok(out) => match guard_complete(rete) {
            Ok(()) => pack(STATUS_OK, results_envelope_json(&out, "").as_bytes()),
            Err(e) => pack(STATUS_ERR, e.as_bytes()),
        },
        Err(e) => pack(STATUS_ERR, e.to_string().as_bytes()),
    })
}

/// Named graphs of a ranged handle (see [`rete_graphs`]).
#[no_mangle]
pub extern "C" fn rete_handle_graphs(id: u32) -> *mut u8 {
    with_handle(id, |rete| {
        let names = rete.graph_names();
        let mut out = Vec::new();
        out.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for iri in names {
            out.extend_from_slice(&(iri.len() as u32).to_le_bytes());
            out.extend_from_slice(iri.as_bytes());
        }
        pack(STATUS_OK, &out)
    })
}

/// Graph-scoped triple-pattern scan over a ranged handle (see
/// [`rete_scan_in_graph`]). Refuses incomplete results.
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn rete_handle_scan_in_graph(
    id: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
    g_ptr: *const u8,
    g_len: u32,
) -> *mut u8 {
    let term = |ptr, len| -> Result<Option<&str>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(Some)
        }
    };
    let (s, p, o, g) = match (
        term(s_ptr, s_len),
        term(p_ptr, p_len),
        term(o_ptr, o_len),
        term(g_ptr, g_len),
    ) {
        (Ok(s), Ok(p), Ok(o), Ok(g)) => (s, p, o, g),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    with_handle(id, |rete| {
        let rows = rete.query_in_graph(g, s, p, o);
        match guard_complete(rete) {
            Ok(()) => pack(STATUS_OK, &frame_triples(&rows)),
            Err(e) => pack(STATUS_ERR, e.as_bytes()),
        }
    })
}

/// All-graphs quad scan over a ranged handle (see [`rete_scan_quads`]). Refuses
/// incomplete results.
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[no_mangle]
pub unsafe extern "C" fn rete_handle_scan_quads(
    id: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
) -> *mut u8 {
    let term = |ptr, len| -> Result<Option<&str>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(Some)
        }
    };
    let (s, p, o) = match (term(s_ptr, s_len), term(p_ptr, p_len), term(o_ptr, o_len)) {
        (Ok(s), Ok(p), Ok(o)) => (s, p, o),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    with_handle(id, |rete| {
        let quads = rete.query_quads(s, p, o);
        if let Err(e) = guard_complete(rete) {
            return pack(STATUS_ERR, e.as_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(quads.len() as u32).to_le_bytes());
        for ((rs, rp, ro), graph) in &quads {
            for t in [rs, rp, ro] {
                out.extend_from_slice(&(t.len() as u32).to_le_bytes());
                out.extend_from_slice(t.as_bytes());
            }
            let g = graph.as_deref().unwrap_or("");
            out.extend_from_slice(&(g.len() as u32).to_le_bytes());
            out.extend_from_slice(g.as_bytes());
        }
        pack(STATUS_OK, &out)
    })
}

// --- streaming scan cursors --------------------------------------------------
//
// `rete_handle_scan_quads` above answers a scan by building the WHOLE result
// inside linear memory before the host sees a row. For a bounded pattern that is
// fine; for `(?s ?p ?o)` it is the wall issue #115 is about — RDF4J asks a Sail
// for exactly that pattern to answer `SELECT ?s ?p ?o … LIMIT 1`, because the
// LIMIT sits above the triple source and the Sail never sees it. On a
// 26-million-quad file that meant the engine exhausted wasm32's address space
// before the first row crossed the boundary.
//
// A cursor fixes it: open once, pull bounded batches, close. The state that has
// to survive between calls is deliberately NOT a suspended Rust iterator (which
// would have to borrow the `Rete` stored beside it — a self-referential struct
// whose soundness rests on drop order and on this crate's lazily-faulted caches
// staying write-once). It is a `u64` resume token plus a graph slot, exactly as
// `Rete::query_batch` was built to take. So a cursor holds no borrow, an
// abandoned cursor leaks a few dozen bytes rather than pinning the engine, and
// closing the handle can drop every cursor on it unconditionally.

/// How many named graphs one `rete_handle_scan_next` may skip past before
/// returning an empty (non-final) batch. A quads scan visits the default graph
/// and then every named graph; a file with hundreds of thousands of graphs that
/// the pattern misses would otherwise make one call run unboundedly long. The
/// host just calls again.
const GRAPH_SKIP_BUDGET: usize = 64;

/// A suspended scan over one open handle: the pattern, where in the graph
/// sequence it is, and the opaque per-graph resume token.
struct ScanCursor {
    handle: u32,
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
    /// `true` = the quads scan: the default graph, then every named graph.
    /// `false` = one graph only, named by `graph`.
    all_graphs: bool,
    /// Named-graph IRIs, materialized only once the default graph is exhausted —
    /// a scan that stops in the default graph never walks the graph directory.
    names: Option<Vec<String>>,
    /// 0 = default graph; `i` = `names[i - 1]`.
    slot: usize,
    /// Single-graph mode's target (`None` = the default graph).
    graph: Option<String>,
    cursor: u64,
    done: bool,
}

thread_local! {
    static CURSORS: std::cell::RefCell<std::collections::HashMap<u32, ScanCursor>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static NEXT_CURSOR: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

/// Open a streaming scan over a ranged handle. `all_graphs != 0` scans the
/// default graph and every named graph, tagging each row with its graph (the
/// quad scan); otherwise the scan is scoped to `g` — zero-length for the default
/// graph, a named-graph IRI otherwise.
///
/// The success payload is the 4-byte LE cursor id. Pull with
/// [`rete_handle_scan_next`] until it reports done, and release with
/// [`rete_handle_scan_close`] — or let [`rete_handle_close`] take it.
///
/// # Safety
/// Every pointer/length pair must describe readable module memory.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub unsafe extern "C" fn rete_handle_scan_open(
    id: u32,
    s_ptr: *const u8,
    s_len: u32,
    p_ptr: *const u8,
    p_len: u32,
    o_ptr: *const u8,
    o_len: u32,
    g_ptr: *const u8,
    g_len: u32,
    all_graphs: u32,
) -> *mut u8 {
    let term = |ptr, len| -> Result<Option<String>, std::str::Utf8Error> {
        if len == 0 {
            Ok(None)
        } else {
            std::str::from_utf8(borrow(ptr, len)).map(|t| Some(t.to_string()))
        }
    };
    let (s, p, o, g) = match (
        term(s_ptr, s_len),
        term(p_ptr, p_len),
        term(o_ptr, o_len),
        term(g_ptr, g_len),
    ) {
        (Ok(s), Ok(p), Ok(o), Ok(g)) => (s, p, o, g),
        _ => return pack(STATUS_ERR, b"pattern term is not utf-8"),
    };
    if !HANDLES.with(|h| h.borrow().contains_key(&id)) {
        return pack(STATUS_ERR, b"invalid or closed ranged handle");
    }
    let cursor = ScanCursor {
        handle: id,
        s,
        p,
        o,
        all_graphs: all_graphs != 0,
        names: None,
        slot: 0,
        graph: g,
        cursor: 0,
        done: false,
    };
    let cid = NEXT_CURSOR.with(|n| {
        let v = n.get();
        n.set(v.wrapping_add(1).max(1));
        v
    });
    CURSORS.with(|c| c.borrow_mut().insert(cid, cursor));
    pack(STATUS_OK, &cid.to_le_bytes())
}

/// Pull the next batch of at most ~`max_rows` rows from a scan cursor. Payload:
///
/// ```text
/// [count: u32 LE][done: u32 LE] then count × ( 3 × [termLen][term] , [graphLen][graph] )
/// ```
///
/// A zero-length graph field is the default graph. `done != 0` means the scan is
/// finished and no further call is needed. `max_rows` is a floor — a batch ends
/// on a group boundary — and a batch may be empty while `done` is 0 (the call
/// skipped past its budget of empty graphs); the host simply calls again.
#[no_mangle]
pub extern "C" fn rete_handle_scan_next(cursor_id: u32, max_rows: u32) -> *mut u8 {
    let max_rows = (max_rows as usize).max(1);
    CURSORS.with(|cursors| {
        let mut cursors = cursors.borrow_mut();
        let Some(cur) = cursors.get_mut(&cursor_id) else {
            return pack(STATUS_ERR, b"invalid or closed scan cursor");
        };
        HANDLES.with(|handles| {
            let handles = handles.borrow();
            let Some(rete) = handles.get(&cur.handle) else {
                return pack(
                    STATUS_ERR,
                    b"the ranged handle this cursor scans was closed",
                );
            };
            match advance(cur, rete, max_rows) {
                Ok((rows, done)) => {
                    let mut out = Vec::new();
                    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
                    out.extend_from_slice(&(done as u32).to_le_bytes());
                    for ((rs, rp, ro), graph) in &rows {
                        for t in [rs, rp, ro] {
                            out.extend_from_slice(&(t.len() as u32).to_le_bytes());
                            out.extend_from_slice(t.as_bytes());
                        }
                        let g = graph.as_deref().unwrap_or("");
                        out.extend_from_slice(&(g.len() as u32).to_le_bytes());
                        out.extend_from_slice(g.as_bytes());
                    }
                    pack(STATUS_OK, &out)
                }
                Err(e) => pack(STATUS_ERR, e.as_bytes()),
            }
        })
    })
}

/// Advance `cur` by one batch, stepping to the next graph when the current one
/// runs out. Returns the rows (each tagged with its graph) and whether the whole
/// scan is finished.
#[allow(clippy::type_complexity)]
fn advance(
    cur: &mut ScanCursor,
    rete: &Rete,
    max_rows: usize,
) -> Result<(Vec<((String, String, String), Option<String>)>, bool), String> {
    let mut skipped = 0usize;
    loop {
        if cur.done {
            return Ok((Vec::new(), true));
        }
        // Which graph is this batch in? Slot 0 is always the default graph, so a
        // scan that finds its rows there never touches the graph directory.
        let graph: Option<String> = if cur.all_graphs {
            if cur.slot == 0 {
                None
            } else {
                let names = cur.names.get_or_insert_with(|| {
                    rete.graph_names().iter().map(|g| g.to_string()).collect()
                });
                match names.get(cur.slot - 1) {
                    Some(name) => Some(name.clone()),
                    None => {
                        cur.done = true;
                        return Ok((Vec::new(), true));
                    }
                }
            }
        } else {
            cur.graph.clone()
        };

        let (rows, next, done) = rete.query_batch(
            graph.as_deref(),
            cur.s.as_deref(),
            cur.p.as_deref(),
            cur.o.as_deref(),
            cur.cursor,
            max_rows,
        );
        guard_complete(rete)?;

        if done {
            // This graph is finished. In quads mode step to the next one.
            let more = cur.all_graphs && {
                let names = cur.names.get_or_insert_with(|| {
                    rete.graph_names().iter().map(|g| g.to_string()).collect()
                });
                cur.slot < names.len()
            };
            if more {
                cur.slot += 1;
                cur.cursor = 0;
            } else {
                cur.done = true;
            }
        } else {
            cur.cursor = next;
        }

        if !rows.is_empty() {
            let done = cur.done;
            return Ok((rows.into_iter().map(|t| (t, graph.clone())).collect(), done));
        }
        if cur.done {
            return Ok((Vec::new(), true));
        }
        // The graph held nothing for this pattern. Move on — but bound how many
        // we walk through in a single call so one FFI call stays short.
        skipped += 1;
        if skipped >= GRAPH_SKIP_BUDGET {
            return Ok((Vec::new(), false));
        }
    }
}

/// Release a scan cursor (idempotent — closing an unknown or already-closed id
/// is a no-op, which is what lets a host reap abandoned cursors without
/// bookkeeping).
#[no_mangle]
pub extern "C" fn rete_handle_scan_close(cursor_id: u32) -> *mut u8 {
    CURSORS.with(|c| c.borrow_mut().remove(&cursor_id));
    pack(STATUS_OK, &[])
}

/// How many scan cursors are currently open (all handles). The host's leak
/// check: an RDF4J `Sail` that forgets one `close()` per query is a slow leak,
/// and this is how the tests prove it does not happen.
#[no_mangle]
pub extern "C" fn rete_open_cursors() -> *mut u8 {
    let n = CURSORS.with(|c| c.borrow().len()) as u32;
    pack(STATUS_OK, &n.to_le_bytes())
}
