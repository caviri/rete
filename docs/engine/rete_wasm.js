/* @ts-self-types="./rete_wasm.d.ts" */

/**
 * A `.rete` opened **once** and kept resident, so a client (the playground's
 * cached/in-memory mode) can run many queries on a big file without re-copying
 * the whole buffer into wasm and re-decoding its dictionary on every call. The
 * methods mirror the free functions above but operate on the already-open
 * [`Rete`]. The few index-free readers (`schema_packed`, `progressive_query`,
 * `check_schema`) stay free functions — they read small ranges from the buffer
 * and are called rarely (once at load / on demand), so a handle buys little.
 */
export class Graph {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        GraphFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_graph_free(ptr, 0);
    }
    /**
     * See [`card`] — the Dataset Card of the resident file.
     * @returns {string | undefined}
     */
    card() {
        const ret = wasm.graph_card(this.__wbg_ptr);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * See [`card_and_build`] — the card and the build record of the resident
     * file, in the same envelope the remote path returns, so one caller
     * handles both sources.
     * @returns {string}
     */
    card_and_build() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.graph_card_and_build(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * See [`file_layout`].
     * @returns {string}
     */
    file_layout() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.graph_file_layout(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`graph_names`].
     * @returns {string}
     */
    graph_names() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.graph_graph_names(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`info`].
     * @returns {string}
     */
    info() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.graph_info(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Open a `.rete` image and keep it resident for repeated querying.
     * @param {Uint8Array} bytes
     */
    constructor(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.graph_new(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        GraphFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * See [`prefix_search`].
     * @param {string} prefix
     * @param {number} limit
     * @returns {string}
     */
    prefix_search(prefix, limit) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.graph_prefix_search(this.__wbg_ptr, ptr0, len0, limit);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * See [`pyramid_tree`].
     * @returns {string}
     */
    pyramid_tree() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.graph_pyramid_tree(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * A **lazy, resumable cursor** over the quads of this graph — the streaming
     * export path. See [`QuadCursor`]; `graph` selects one graph (`""` = the
     * default graph), `None` streams the default graph followed by every named
     * graph. `s` / `p` / `o` optionally restrict the dump to a triple pattern,
     * which **prunes tiles** rather than filtering rows.
     * @param {string | null} [graph]
     * @param {string | null} [s]
     * @param {string | null} [p]
     * @param {string | null} [o]
     * @returns {QuadCursor}
     */
    quads(graph, s, p, o) {
        var ptr0 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(s) ? 0 : passStringToWasm0(s, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(p) ? 0 : passStringToWasm0(p, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        var ptr3 = isLikeNone(o) ? 0 : passStringToWasm0(o, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len3 = WASM_VECTOR_LEN;
        const ret = wasm.graph_quads(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        return QuadCursor.__wrap(ret);
    }
    /**
     * See [`query`].
     * @param {string} query
     * @param {string} format
     * @returns {string}
     */
    query(query, format) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.graph_query(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * See [`query_communities`].
     * @param {string} query
     * @param {number | null} [round]
     * @returns {string}
     */
    query_communities(query, round) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.graph_query_communities(this.__wbg_ptr, ptr0, len0, isLikeNone(round) ? Number.MAX_SAFE_INTEGER : (round) >>> 0);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * As [`query`], with explicit opt-in toggles: `reason` (OWL 2 QL
     * entailment) and `union_default` (union default graph — a pattern
     * outside `GRAPH` matches the merge of the default graph and every named
     * graph, the Virtuoso / GraphDB / Jena TDB mode; non-standard, so plain
     * [`Graph::query`] never does this).
     * @param {string} query
     * @param {string} format
     * @param {boolean} reason
     * @param {boolean} union_default
     * @returns {string}
     */
    query_opts(query, format, reason, union_default) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.graph_query_opts(this.__wbg_ptr, ptr0, len0, ptr1, len1, reason, union_default);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * As [`query`], with OWL 2 QL entailment on (`rdfs:subClassOf` /
     * `subPropertyOf` / `domain` / `range` reasoning by query rewriting).
     * @param {string} query
     * @param {string} format
     * @returns {string}
     */
    query_reasoned(query, format) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.graph_query_reasoned(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * See [`query_triples`].
     * @param {string | null} [subject]
     * @param {string | null} [predicate]
     * @param {string | null} [object]
     * @returns {string}
     */
    query_triples(subject, predicate, object) {
        let deferred5_0;
        let deferred5_1;
        try {
            var ptr0 = isLikeNone(subject) ? 0 : passStringToWasm0(subject, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(predicate) ? 0 : passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            var ptr2 = isLikeNone(object) ? 0 : passStringToWasm0(object, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len2 = WASM_VECTOR_LEN;
            const ret = wasm.graph_query_triples(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            var ptr4 = ret[0];
            var len4 = ret[1];
            if (ret[3]) {
                ptr4 = 0; len4 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    /**
     * See [`reach`].
     * @param {string} predicate
     * @param {string} seeds
     * @param {boolean} reverse
     * @returns {string}
     */
    reach(predicate, seeds, reverse) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(seeds, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.graph_reach(this.__wbg_ptr, ptr0, len0, ptr1, len1, reverse);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * See [`reason`].
     * @param {string | null} [graph]
     * @returns {string}
     */
    reason(graph) {
        let deferred2_0;
        let deferred2_1;
        try {
            var ptr0 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            const ret = wasm.graph_reason(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`schema`] — the live (scanning) profile; prefer `schema_packed` when
     * the file carries a pyramid.
     * @returns {string}
     */
    schema() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.graph_schema(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`shacl`].
     * @param {string} shapes_turtle
     * @param {string | null | undefined} graph
     * @param {string} format
     * @returns {string}
     */
    shacl(shapes_turtle, graph, format) {
        let deferred5_0;
        let deferred5_1;
        try {
            const ptr0 = passStringToWasm0(shapes_turtle, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len2 = WASM_VECTOR_LEN;
            const ret = wasm.graph_shacl(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            var ptr4 = ret[0];
            var len4 = ret[1];
            if (ret[3]) {
                ptr4 = 0; len4 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    /**
     * Byte length of the TEXT_INDEX section, `0` when the file has none — a
     * header read, never a fault. The UI asks this to decide whether to offer
     * full-text search at all, and to state the cost before the first one.
     * `f64` because the section outgrows `u32`: causenet's is 1.88 GB.
     * @returns {number}
     */
    text_index_len() {
        const ret = wasm.graph_text_index_len(this.__wbg_ptr);
        return ret;
    }
    /**
     * Byte length of the TEXT_INDEX's leading **token table** — what a first
     * [`Graph::text_search_one`] actually faults, and therefore the only honest
     * number to quote as its cost. [`Graph::text_index_len`] is the whole
     * section, postings blob included, and overstates it 6.5× on
     * `epfl-infoscience` (195 MB section, 29 MB token table); the postings are
     * only ever fetched one list at a time. `0` when the file has no text index
     * or the length could not be read — the caller must then say nothing about
     * a token table rather than pass the section length off as one.
     * `f64` for the same reason as the section length: causenet's table is
     * 1.88 GB.
     * @returns {number}
     */
    text_index_token_table_len() {
        const ret = wasm.graph_text_index_token_table_len(this.__wbg_ptr);
        return ret;
    }
    /**
     * See [`text_search`].
     * @param {string[]} words
     * @param {string | null | undefined} contains_prefix
     * @param {number} limit
     * @returns {string}
     */
    text_search(words, contains_prefix, limit) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passArrayJsValueToWasm0(words, wasm.__wbindgen_malloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(contains_prefix) ? 0 : passStringToWasm0(contains_prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ret = wasm.graph_text_search(this.__wbg_ptr, ptr0, len0, ptr1, len1, limit);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * [`Graph::text_search`] from ONE phrase: whitespace splits it into words
     * and **every** word must match (AND), like `rete search --contains a b`.
     * One string in, one string out — that is what the remote twin's
     * hand-marshaled asyncify path can carry (a JS array marshaled raw is what
     * traps), and the UI is a single text box either way. Same JSON envelope.
     * @param {string} phrase
     * @param {number} limit
     * @returns {string}
     */
    text_search_one(phrase, limit) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(phrase, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.graph_text_search_one(this.__wbg_ptr, ptr0, len0, limit);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * See [`why_triples`].
     * @param {string | null} [subject]
     * @param {string | null} [predicate]
     * @param {string | null} [object]
     * @returns {string}
     */
    why_triples(subject, predicate, object) {
        let deferred5_0;
        let deferred5_1;
        try {
            var ptr0 = isLikeNone(subject) ? 0 : passStringToWasm0(subject, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(predicate) ? 0 : passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            var ptr2 = isLikeNone(object) ? 0 : passStringToWasm0(object, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len2 = WASM_VECTOR_LEN;
            const ret = wasm.graph_why_triples(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            var ptr4 = ret[0];
            var len4 = ret[1];
            if (ret[3]) {
                ptr4 = 0; len4 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
}
if (Symbol.dispose) Graph.prototype[Symbol.dispose] = Graph.prototype.free;

/**
 * A **lazy, resumable cursor** over the quads of an open `.rete` — the engine
 * side of `for await (const [s, p, o, g] of graph.dump())` in the JS client.
 *
 * # Why a cursor and not a callback
 *
 * [`Rete::dump_each`] already streams in constant memory, but a Rust callback
 * cannot be *paused* to hand control back to JavaScript: to feed a JS iterator
 * it would have to buffer every quad first, which is exactly the `Vec` that
 * [`Rete::dump`] builds and that OOMs on a large file. This wraps
 * [`Rete::query_batch`] instead, so the scan can be suspended between calls and
 * resumed in place — the whole resume state is one opaque `u64`, never a
 * whole-graph materialization anywhere in the pipeline.
 *
 * # Why batched (and not one call per quad)
 *
 * Each wasm→JS call costs far more than decoding a triple, and every returned
 * `String` becomes a fresh JS string. Pulling one quad per call would make the
 * boundary the bottleneck; pulling *all* of them would reintroduce the `Vec`.
 * So the JS wrapper asks for `DUMP_BATCH` quads at a time and yields them one
 * by one — bounded, amortized, and lazy. Memory is O(batch), not O(graph).
 *
 * # Cost model
 *
 * The dictionary is **not** prefetched whole: each batch faults only the
 * chunks its own terms live in, so taking five quads off the front costs five
 * quads' worth of dictionary rather than all of it. Index tiles fault in as the
 * scan advances and stay resident, and so do dictionary chunks, so an
 * **unfiltered** dump driven to the end still ends up fetching essentially the
 * whole file — that is inherent in exporting a graph.
 *
 * A **filtered** cursor (a bound `s` / `p` / `o`) is a different shape: the
 * scan routes to the one permutation that sorts on the bound prefix and drops
 * every tile whose synopsis proves it cannot match, from the tile directory,
 * *without fetching it*. On `cordis.rete` (801 MB, six named graphs) dumping
 * one predicate of one graph reads 16 MB where the unfiltered dump of that
 * graph reads 376 MB. Peak memory is O(faulted dictionary + index), never
 * O(quads), either way.
 */
export class QuadCursor {
    static __wrap(ptr) {
        const obj = Object.create(QuadCursor.prototype);
        obj.__wbg_ptr = ptr;
        QuadCursorFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        QuadCursorFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_quadcursor_free(ptr, 0);
    }
    /**
     * Whether every selected graph has been streamed to its end.
     * @returns {boolean}
     */
    done() {
        const ret = wasm.quadcursor_done(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Up to `max` quads as a **flat** `string[]` of `[s, p, o, g, s, p, o, g, …]`
     * N-Triples term tokens, `g` being `""` for the default graph. Flat because
     * a nested array would allocate one JS array per quad for no gain — the
     * caller slices it into tuples as it yields them.
     *
     * An empty array means the stream is finished; keep calling until you get
     * one (that final call is what verifies no range fetch failed mid-dump).
     * @param {number | null} [max]
     * @returns {string[]}
     */
    next_batch(max) {
        const ret = wasm.quadcursor_next_batch(this.__wbg_ptr, isLikeNone(max) ? Number.MAX_SAFE_INTEGER : (max) >>> 0);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Up to `max` quads already serialized as N-Quads lines in **one** string —
     * the `.rete` → Oxigraph / N-Quads-file path. One string crossing per batch
     * instead of four per quad: no per-term JS string, no re-serialization in
     * JavaScript, and the terms are already canonical N-Triples tokens, so the
     * lines are emitted verbatim.
     *
     * An empty string means the stream is finished.
     * @param {number | null} [max]
     * @returns {string}
     */
    next_nquads(max) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.quadcursor_next_nquads(this.__wbg_ptr, isLikeNone(max) ? Number.MAX_SAFE_INTEGER : (max) >>> 0);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
}
if (Symbol.dispose) QuadCursor.prototype[Symbol.dispose] = QuadCursor.prototype.free;

/**
 * A remote `.rete` opened **once over HTTP range** and kept resident in the
 * worker, so repeated queries on the same URL reuse (a) the block cache — any
 * 64 KiB block fetched once is served from memory by [`BlockCacheReader`] — and
 * (b) the lazily faulted index tiles + decoded dictionary chunks that live
 * inside the resident [`Rete`]. The free [`sparql_url`] re-opens the file on
 * every call, so its block cache dies after one query; this handle keeps it.
 * The counting reader stays reachable so the worker can read cumulative
 * bytes/requests and show how little a cache-hit query actually fetched.
 * **Worker-only** (synchronous range-read XHR).
 */
export class RemoteGraph {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RemoteGraphFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_remotegraph_free(ptr, 0);
    }
    /**
     * See [`card_url`] — the Dataset Card, over the resident handle's reader
     * (so the header range it already fetched is served from the block cache).
     * @returns {string | undefined}
     */
    card() {
        const ret = wasm.remotegraph_card(this.__wbg_ptr);
        if (ret[3]) {
            throw takeFromExternrefTable0(ret[2]);
        }
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * See [`card_and_build_url`] — card + build record over the resident
     * handle's reader, still one coalesced range (and served from the block
     * cache when the header range is already there).
     * @returns {string}
     */
    card_and_build() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.remotegraph_card_and_build(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * The file's content hash (blake3-16, hex). The worker keys its session
     * cache by this rather than the URL, so two URLs of the same file share the
     * cache — and it's the stable key a future IndexedDB block store (L3) needs
     * to survive page reloads.
     * @returns {string}
     */
    content_hash() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.remotegraph_content_hash(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * See [`graph_names`].
     * @returns {string}
     */
    graph_names() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.remotegraph_graph_names(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`info`] — read from the resident header, no extra fetch.
     * @returns {string}
     */
    info() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.remotegraph_info(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Open a remote `.rete` over HTTP range and keep it resident for repeated
     * querying. The first query faults in the dictionary chunks + index tiles it
     * needs; later queries on this handle reuse them and the block cache.
     * @param {string} url
     */
    constructor(url) {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.remotegraph_new(ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        RemoteGraphFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * See [`prefix_search`] — over the resident, cached remote handle. Faults the
     * pyramid (where the label index lives) on the first call, then serves the
     * search from memory.
     * @param {string} prefix
     * @param {number} limit
     * @returns {string}
     */
    prefix_search(prefix, limit) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_prefix_search(this.__wbg_ptr, ptr0, len0, limit);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * See [`Graph::quads`] — the SAME lazy cursor, over the lazily range-read
     * remote handle.
     *
     * An **unfiltered** dump is not network-lazy and cannot be: it resolves
     * every term and visits every tile, so it ends up fetching essentially the
     * whole file (and what it faults stays resident). A **filtered** one is:
     * pass `s` / `p` / `o` and the scan routes to one permutation, keeps only
     * the tiles whose synopsis admits the bound components, and fetches those.
     * On `cordis.rete` (801 MB) one predicate of one named graph costs 16 MB
     * instead of 376 MB. To peek at an unfiltered graph, still prefer a `LIMIT`
     * query. Worker-only in the browser, like every other read here.
     * @param {string | null} [graph]
     * @param {string | null} [s]
     * @param {string | null} [p]
     * @param {string | null} [o]
     * @returns {QuadCursor}
     */
    quads(graph, s, p, o) {
        var ptr0 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(s) ? 0 : passStringToWasm0(s, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(p) ? 0 : passStringToWasm0(p, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        var ptr3 = isLikeNone(o) ? 0 : passStringToWasm0(o, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len3 = WASM_VECTOR_LEN;
        const ret = wasm.remotegraph_quads(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        return QuadCursor.__wrap(ret);
    }
    /**
     * See [`sparql_url`] — same query, but over the resident, cached handle.
     * The incompleteness verdict is PER QUERY: reset the sticky failure flags
     * first, so one transient fetch failure fails only the query it happened
     * in — not every later query on this session (failed tiles/chunks are
     * never cached, so they retry here).
     * @param {string} query
     * @param {string} format
     * @returns {string}
     */
    query(query, format) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_query(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * As [`query`], with explicit opt-in toggles — see [`Graph::query_opts`].
     * With `union_default` on, a lazy remote read may fault the index tiles of
     * every named graph the union touches (the merge is strictly opt-in).
     * @param {string} query
     * @param {string} format
     * @param {boolean} reason
     * @param {boolean} union_default
     * @returns {string}
     */
    query_opts(query, format, reason, union_default) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_query_opts(this.__wbg_ptr, ptr0, len0, ptr1, len1, reason, union_default);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * As [`query`], with OWL 2 QL entailment on (reason over the ontology while
     * reading only the bytes the rewritten query touches).
     * @param {string} query
     * @param {string} format
     * @returns {string}
     */
    query_reasoned(query, format) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_query_reasoned(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * See [`schema_url`] — the **baked** schema pyramid over the resident
     * handle. Deliberately never falls back to the scanning [`schema`]: that
     * would drag the whole remote file across the wire.
     * @returns {string}
     */
    schema() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.remotegraph_schema(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * See [`shacl_url`] — validate over the resident handle. The default graph
     * validates lazily against the index (only the shapes' targets are
     * fetched), so a shape over a huge remote file stays cheap.
     * @param {string} shapes_turtle
     * @param {string | null | undefined} graph
     * @param {string} format
     * @returns {string}
     */
    shacl(shapes_turtle, graph, format) {
        let deferred5_0;
        let deferred5_1;
        try {
            const ptr0 = passStringToWasm0(shapes_turtle, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len2 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_shacl(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            var ptr4 = ret[0];
            var len4 = ret[1];
            if (ret[3]) {
                ptr4 = 0; len4 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    /**
     * `{ fileLength, bytes, requests, base }` — CUMULATIVE physical fetches
     * since this session opened. The worker diffs successive calls to report a
     * single query's traffic (a fully cached re-run adds ~0). `fileLength` is
     * the **graph's** length and `base` the byte offset it starts at: `0` for an
     * ordinary `.rete`, and the size of the HTML shell for a polyglot file.
     * @returns {string}
     */
    stats() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.remotegraph_stats(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * See [`Graph::text_index_len`] — read from the resident header, so it
     * costs no fetch at all. Worth asking before [`RemoteGraph::text_search`]:
     * it is the size of the section the first search starts pulling over the
     * wire, so the UI can warn instead of surprising the user.
     * @returns {number}
     */
    text_index_len() {
        const ret = wasm.remotegraph_text_index_len(this.__wbg_ptr);
        return ret;
    }
    /**
     * See [`Graph::text_index_token_table_len`] — the figure to quote before
     * [`RemoteGraph::text_search`], because it is what that first search pulls
     * over the wire; the section length would promise the user several times
     * the real bill.
     *
     * Unlike [`RemoteGraph::text_index_len`] this is not free: the token
     * table's length lives in the section's first bytes, not the header, so it
     * costs ONE ≤10-byte range read (memoized). Trivial next to the table it
     * measures — but it *is* IO, so the asyncify path must drive this call
     * rather than treat it as a header field.
     * @returns {number}
     */
    text_index_token_table_len() {
        const ret = wasm.remotegraph_text_index_token_table_len(this.__wbg_ptr);
        return ret;
    }
    /**
     * See [`text_search`] — over the resident remote handle. Faults the TEXT_INDEX
     * token table on the first call, then fetches only the queried posting lists
     * (never the whole postings blob), serving repeat searches from memory.
     * @param {string[]} words
     * @param {string | null | undefined} contains_prefix
     * @param {number} limit
     * @returns {string}
     */
    text_search(words, contains_prefix, limit) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passArrayJsValueToWasm0(words, wasm.__wbindgen_malloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(contains_prefix) ? 0 : passStringToWasm0(contains_prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_text_search(this.__wbg_ptr, ptr0, len0, ptr1, len1, limit);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * See [`Graph::text_search_one`] — over the resident remote handle, with
     * the same token-table-then-posting-lists fault pattern as
     * [`RemoteGraph::text_search`]. This is the shape the playground's raw
     * asyncify glue drives: one string in, one string out, marshaled once.
     * @param {string} phrase
     * @param {number} limit
     * @returns {string}
     */
    text_search_one(phrase, limit) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(phrase, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_text_search_one(this.__wbg_ptr, ptr0, len0, limit);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
}
if (Symbol.dispose) RemoteGraph.prototype[Symbol.dispose] = RemoteGraph.prototype.free;

/**
 * Module init: route Rust panics to `console.error` with their message and
 * location. In release wasm a panic otherwise aborts as a bare
 * `RuntimeError: unreachable` with no clue where — this turns that into a
 * `rete-wasm panic: panicked at '…', src/…:line` line in the devtools console,
 * so an intermittent first-query crash (e.g. a parser tripping on a flaky
 * range read) can actually be diagnosed.
 */
export function __start() {
    wasm.__start();
}

/**
 * Build a complete `.rete` file image from RDF text, entirely in the browser.
 *
 * `format` is `"nt"` (N-Triples), `"nq"` (N-Quads; named graphs become a
 * dataset), or `"ttl"` (Turtle). Returns the file bytes (a `Uint8Array`),
 * ready to download or to hand straight back to the query functions. The wasm
 * build has no zstd *encoder*, so sections are written uncompressed (codec
 * `NONE`) — larger than a CLI build of the same data, but every reader
 * accepts it; rebuild with `rete build` for a compressed file.
 * @param {string} text
 * @param {string} format
 * @returns {Uint8Array}
 */
export function build(text, format) {
    const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ret = wasm.build(ptr0, len0, ptr1, len1);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v3 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v3;
}

/**
 * [`build`], but the file carries a **Dataset Card** written from
 * `card_json` — the same document `rete build --card-file` takes, validated
 * by the same rules ([`rete_core::card::validate_curated_card`]), so a card
 * authored in the browser is one the CLI would also have accepted.
 *
 * What the browser can and cannot put in a card, stated plainly because the
 * difference matters to whoever reads the file afterwards:
 *
 * - **Curated fields travel in full** — title, description, licence, source,
 *   version, creators, publisher, DOI, citation, keywords, theme, the `extra`
 *   bag, everything on [`rete_core::card::CURATED_CARD_FIELDS`].
 * - **The four counts are measured, not asserted**: `triple_count`,
 *   `quad_count`, `named_graph_count` and `term_count` come from the build's
 *   own [`BuildStats`](rete_core::ingest::BuildStats), and any values supplied
 *   for them would be ignored (they are not curated fields, so supplying them
 *   is already an error). `format_version` is stamped by the writer.
 * - **The derived profile is NOT written.** Predicates, classes,
 *   vocabularies, datatypes, languages, class links, hubs, signals and the
 *   tiered starter-query library are derived by `rete-cli`, which this crate
 *   does not depend on. Their absence is honest absence: the card simply does
 *   not carry those keys, exactly as a `rete merge` card does not. Rebuild
 *   with `rete build --card-file` to get them.
 * - **No build-info section** (kind 7) is written: its cost figures come from
 *   measuring the starter queries, and there are none to measure.
 *
 * Pass an empty string for no card — byte-identical to [`build`].
 * @param {string} text
 * @param {string} format
 * @param {string} card_json
 * @returns {Uint8Array}
 */
export function build_with_card(text, format, card_json) {
    const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len1 = WASM_VECTOR_LEN;
    const ptr2 = passStringToWasm0(card_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len2 = WASM_VECTOR_LEN;
    const ret = wasm.build_with_card(ptr0, len0, ptr1, len1, ptr2, len2);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    var v4 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
    wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    return v4;
}

/**
 * The embedded **Dataset Card** — the file's own self-description (title,
 * description, license, provenance, counts, example queries) as the JSON text
 * it was written with, or `undefined` when the file carries none. Reads the
 * metadata section straight out of the buffer.
 * @param {Uint8Array} bytes
 * @returns {string | undefined}
 */
export function card(bytes) {
    const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.card(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * [`card_and_build_url`] for an image already in memory — no I/O at all.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function card_and_build(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.card_and_build(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The Dataset Card **and the build record** of a remote `.rete`, in the same
 * budget as the card alone: one header read, then **one coalesced range**
 * covering both sections — the writer lays the kind-7 build-info immediately
 * after the metadata precisely so this holds
 * ([`rete_core::range::read_card_and_build_info_ranged`], pinned by a
 * `rete-core` test). Reading the two separately would have made the CARD tier
 * cost three requests instead of two, which is why there is one export rather
 * than a second `build_info_url`.
 *
 * JSON envelope:
 * `{"schemaVersion":1,"card":<text|null>,"build":<text|null>,"text_index":{…}}`.
 * `card` and `build` are the sections' **own bytes** as text, not a
 * re-serialization — the card a client displays is the card the file holds.
 * `text_index` is the one thing the file does *not* store about itself and this
 * reader measures instead (see the private `text_index_json`). Worker-only
 * (synchronous XHR).
 * @param {string} url
 * @returns {string}
 */
export function card_and_build_url(url) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.card_and_build_url(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The embedded **Dataset Card of a remote `.rete`**, in **two small range
 * requests**: the header, then the metadata section it points at — never the
 * dictionary, index, or pyramid. This is the index-free CARD tier: a client
 * learns what a multi-gigabyte graph *is* for a few KB. `undefined` when the
 * file carries no card. Worker-only (synchronous XHR).
 * @param {string} url
 * @returns {string | undefined}
 */
export function card_url(url) {
    const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.card_url(ptr0, len0);
    if (ret[3]) {
        throw takeFromExternrefTable0(ret[2]);
    }
    let v2;
    if (ret[0] !== 0) {
        v2 = getStringFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
    }
    return v2;
}

/**
 * **Index-free schema coherence (Tier-0)** over an in-memory `.rete`: read only
 * the header + pyramid-meta (never the dictionary or the triple index) and report
 * schema-level incoherent points (subClassOf cycles, unsatisfiable classes).
 * Errors if the file ships no schema pyramid.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function check_schema(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.check_schema(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * **Index-free schema coherence (Tier-0) over a remote `.rete` URL.** Reads only
 * TWO ranges — the header and the trailing schema block (the header records its
 * length) — never the dictionary, the community summary, or the triple index. So
 * it's a flat **~1–8 KB at any graph size** (8.1 KB of a 48.8 MB file; see
 * docs/BENCHMARK.md), making it the cheap "is the ontology coherent?" gate.
 * Worker-only (synchronous XHR); a failed range fetch is an error, never a false
 * "coherent".
 * @param {string} url
 * @returns {string}
 */
export function check_schema_url(url) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.check_schema_url(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Recompute the Louvain community decomposition and report, per community, its
 * member-subject count and triple count. Powers the "split by community"
 * strategy view in the playground. JSON:
 * `[{ "community": N, "size": M, "triples": K }, ...]`, ordered by community id.
 *
 * `round` selects the dendrogram granularity; `None` chooses the round for the
 * default tile budget (the same choice the native build makes).
 * @param {Uint8Array} bytes
 * @param {number | null} [round]
 * @returns {string}
 */
export function communities(bytes, round) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.communities(ptr0, len0, isLikeNone(round) ? Number.MAX_SAFE_INTEGER : (round) >>> 0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The file's byte layout, for the playground's byte-map view. JSON:
 * `{ "fileLength": N, "segments": [ { "kind", "label", "offset", "len" } ] }`
 * — segments sorted by offset; uncovered bytes are container framing.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function file_layout(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.file_layout(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The **true byte length of a remote `.rete`**, in 1–2 tiny range requests —
 * derived from the file's *own* header (the issue-#95 probe: sections are
 * back-to-back and the file ends with the 4-byte `RETE` footer), never from
 * the transport's numbers, which may describe a compressed representation
 * (GitHub Pages HEADs a 71 MB file as its 58 MB gzip) or be hidden from
 * cross-origin JS entirely. This is how a UI can say what "download the whole
 * file" actually costs **before** committing to it.
 * JSON: `{ "schemaVersion": 1, "fileLength": <bytes> }`. Worker-only
 * (synchronous XHR in the sync build).
 * @param {string} url
 * @returns {string}
 */
export function file_len_url(url) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.file_len_url(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Drop a registration made by [`register_local_file`]. Releases this wasm
 * instance's reference to the `Blob`; any open handle over it stops working.
 * @param {string} url
 * @returns {boolean}
 */
export function forget_local_file(url) {
    const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.forget_local_file(ptr0, len0);
    return ret !== 0;
}

/**
 * The named-graph IRIs of a dataset, as a JSON array.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function graph_names(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.graph_names(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Parse the fixed-size header and report the byte ranges a *progressive*
 * client needs for the overview — the dictionary and the pyramid summary — plus
 * the (large) index range it can skip, and the metadata (Dataset Card) range.
 * JSON: `{ "dictOffset","dictLen","pyramidOffset","pyramidLen","indexOffset",
 * "indexLen","metadataOffset","metadataLen" }`.
 * The browser fetches bytes `0..HEADER_LEN`, calls this, then range-fetches only the
 * dict + pyramid — never the index. A host with its own byte reader (Node over a
 * local file, say) can use `metadataOffset`/`metadataLen` to read just the card.
 * @param {Uint8Array} head
 * @returns {string}
 */
export function header_ranges(head) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(head, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.header_ranges(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The wasm linear memory's current size in bytes — the engine's high-water
 * mark, since wasm memory grows but never shrinks. Exposed so a host can
 * *measure* the streaming-dump memory claim instead of trusting it: sample it
 * before and after a full [`QuadCursor`] drain and the growth stays flat
 * however many quads went by, where materializing them all does not.
 * @returns {number}
 */
export function heap_bytes() {
    const ret = wasm.heap_bytes();
    return ret;
}

/**
 * Header summary as JSON: `{ "quads": N, "terms": N, "pyramidLevels": N }`.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function info(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.info(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Prefix-search the label index of an embedded `.rete` image: the subjects whose
 * label starts with `prefix` (case-insensitive), as `[{"label":…,"subject":…}]`,
 * capped at `limit`. Answers from the bounded label-index block in the
 * pyramid-meta — no literal scan. Empty array when the file has no label index.
 * @param {Uint8Array} bytes
 * @param {string} prefix
 * @param {number} limit
 * @returns {string}
 */
export function prefix_search(bytes, prefix, limit) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.prefix_search(ptr0, len0, ptr1, len1, limit);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Answer a conservative subset of SPARQL exactly from the pyramid summary,
 * without opening the triple index. Unsupported query shapes return an error
 * instead of silently falling back to a full scan.
 * @param {Uint8Array} bytes
 * @param {string} query
 * @returns {string}
 */
export function progressive_query(bytes, query) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.progressive_query(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * The full community pyramid as a tree — the "cluster of clusters" view.
 * Per dendrogram round (index 0 = finest, last = coarsest), every community
 * with its member-node count, triple count (triples whose subject belongs to
 * it), and its parent community at the next-coarser round (`null` at the
 * top). JSON:
 * `{ "rounds": N, "levels": [ [ { "id", "nodes", "triples", "parent" } ] ] }`.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function pyramid_tree(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.pyramid_tree(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Run any SPARQL form (SELECT / ASK / CONSTRUCT / DESCRIBE) and serialize the
 * result for the playground. Returns a JSON string the page parses; a `"kind"`
 * field (`"select"|"ask"|"construct"`) tells the UI how to render it.
 *
 * `format` controls the construct serialization and the select fallback:
 * - SELECT → `{ "kind":"select", "vars":[...], "rows":[ {var:value,...} ] }`
 *   (`format` is ignored for SELECT — the table view is always available).
 * - ASK → `{ "kind":"ask", "boolean": true|false }`.
 * - CONSTRUCT/DESCRIBE →
 *   - `format=="ttl"`    → `{ "kind":"construct", "format":"ttl",    "text": "<turtle>" }`
 *   - `format=="jsonld"` → `{ "kind":"construct", "format":"jsonld", "text": "<json-ld>" }`
 *   - otherwise (`table`/`json`) → `{ "kind":"construct", "triples": [[s,p,o], ...] }`.
 * @param {Uint8Array} bytes
 * @param {string} query
 * @param {string} format
 * @returns {string}
 */
export function query(bytes, query, format) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.query(ptr0, len0, ptr1, len1, ptr2, len2);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Evaluate a SELECT with the **community-split strategy**: every basic graph
 * pattern is decomposed into subject stars, each star is evaluated per
 * pyramid community (the members pushed in as a VALUES binding), and the
 * stars are recombined with global hash joins — so multi-hop joins work and
 * cross-community solutions survive. FILTER / UNION / OPTIONAL / MINUS
 * recurse; paths and GRAPH blocks evaluate globally inside the split; GROUP
 * BY / ORDER BY / LIMIT / DISTINCT run once on the merged rows. Answers are
 * identical to [`query`]'s. Refused only when nothing in the query can
 * split (no BGP with a variable subject) or for FROM / FROM NAMED. JSON:
 * the SELECT envelope plus `"communities": [{ "community", "subjects",
 * "rows" }, …]` (rows contributed per community across all split stars).
 * @param {Uint8Array} bytes
 * @param {string} query
 * @param {number | null} [round]
 * @returns {string}
 */
export function query_communities(bytes, query, round) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.query_communities(ptr0, len0, ptr1, len1, isLikeNone(round) ? Number.MAX_SAFE_INTEGER : (round) >>> 0);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Run a SPARQL SELECT; returns a JSON array of solution objects
 * (`{ "var": "value", ... }`).
 * @param {Uint8Array} bytes
 * @param {string} query
 * @returns {string}
 */
export function query_sparql(bytes, query) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.query_sparql(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Evaluate a triple pattern; `null`/`undefined` positions are wildcards.
 * Returns a JSON array of `[subject, predicate, object]` triples.
 * @param {Uint8Array} bytes
 * @param {string | null} [subject]
 * @param {string | null} [predicate]
 * @param {string | null} [object]
 * @returns {string}
 */
export function query_triples(bytes, subject, predicate, object) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(subject) ? 0 : passStringToWasm0(subject, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate) ? 0 : passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        var ptr3 = isLikeNone(object) ? 0 : passStringToWasm0(object, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len3 = WASM_VECTOR_LEN;
        const ret = wasm.query_triples(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

/**
 * Multi-source transitive reachability over one relation, run **serially**
 * (the browser engine is single-threaded — the native CLI's
 * `rete reach --parallel` fans one task per seed). For each seed, the set of
 * nodes it transitively reaches; with `reverse`, the set that reaches it
 * (impact analysis).
 *
 * - `predicate` — the relation IRI token, e.g. `<http://ex/dependsOn>`.
 * - `seeds` — a JSON array of seed IRI tokens (e.g. `["<http://ex/app>"]`); a
 *   bare single IRI string is also accepted.
 * - `reverse` — traverse edges backward ("who reaches the seed?").
 *
 * Returns a JSON array, one entry per seed in input order:
 * `[{ "seed":"<iri>", "reached":["<iri>",...], "count":N }, ...]`.
 * A seed not present in the graph yields `{ "seed":"...", "error":"not in graph" }`
 * instead of failing the whole call.
 * @param {Uint8Array} bytes
 * @param {string} predicate
 * @param {string} seeds
 * @param {boolean} reverse
 * @returns {string}
 */
export function reach(bytes, predicate, seeds, reverse) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(seeds, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.reach(ptr0, len0, ptr1, len1, ptr2, len2, reverse);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Multi-source reachability over a **remote** `.rete` URL (lazy HTTP range
 * reads): builds adjacency for `predicate` by faulting only that predicate's
 * tiles, then BFS from each seed. Worker-only (synchronous XHR).
 * @param {string} url
 * @param {string} predicate
 * @param {string} seeds
 * @param {boolean} reverse
 * @returns {string}
 */
export function reach_url(url, predicate, seeds, reverse) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(seeds, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.reach_url(ptr0, len0, ptr1, len1, ptr2, len2, reverse);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Run the OWL RL / RDFS reasoner over an **in-memory** `.rete` and report the
 * inferred-triple count plus any incoherent points (logical contradictions).
 * `graph` selects a named graph; the default graph is used when omitted. This is
 * the complete (Tier-2) check — it materializes the whole graph. Returns the
 * `reasoning_json` envelope (no `remote` block).
 * @param {Uint8Array} bytes
 * @param {string | null} [graph]
 * @returns {string}
 */
export function reason(bytes, graph) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.reason(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * **Selective (Tier-1) coherence check** over a remote `.rete`: evaluate a
 * CONSTRUCT (touching only the tiles its constant-predicate patterns need), then
 * reason over just that subgraph. Pass [`COHERENCE_CONSTRUCT`] for the standard
 * class/equality coherence slice, or a custom CONSTRUCT to scope the check
 * further. Unlike [`reason_url`] (which materializes the whole graph), this
 * fetches only the slice the CONSTRUCT selects. Worker-only (synchronous XHR).
 * @param {string} url
 * @param {string} construct
 * @returns {string}
 */
export function reason_construct_url(url, construct) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(construct, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.reason_construct_url(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Run the reasoner over a **remote** `.rete` URL — the complete (Tier-2) check.
 * It materializes the whole graph, so this faults in the dataset's chunks/tiles
 * as it reads (≈ the whole file); use [`reason_construct_url`] for the cheaper
 * selective check. Worker-only (synchronous XHR). A failed range fetch mid-read
 * is an error, never a silently-incomplete (and thus possibly false "coherent")
 * result. JSON adds `"remote": { fileLength, bytes, requests }`.
 * @param {string} url
 * @param {string | null} [graph]
 * @returns {string}
 */
export function reason_url(url, graph) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.reason_url(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Register a local `File`/`Blob` under a `rete-local:…` URL, so every `*_url`
 * entry point can range-read it.
 *
 * **Worker-only** (the read uses `FileReaderSync`), and the caller mints the
 * URL: a worker can be torn down and rebuilt — the playground does that on a
 * wasm trap, an engine switch, or a phone memory reclaim — and the page must be
 * able to re-register the same file under the same URL so a resident session
 * key stays stable. Re-registering an existing URL replaces the blob.
 * @param {string} url
 * @param {Blob} blob
 */
export function register_local_file(url, blob) {
    const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.register_local_file(ptr0, len0, blob);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

/**
 * The ontology profile (the semantic coarse graph), as JSON:
 * `{ "classes": [["<iri>", count], ...],
 *    "relations": [["sClass","pred","oClass", count], ...] }`.
 * The "overview first" payload — render it before fetching any detail.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function schema(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.schema(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Read the **baked** schema pyramid (classes + class-level relations) straight
 * from the file's schema block via a slice reader — no triple scan. The
 * in-memory twin of [`schema_url`]: a cached big graph gets its schema from a
 * few KB of the buffer instead of dumping every triple (seconds on a 150 MB
 * file). Errors when the file carries no schema pyramid, so callers fall back
 * to the scanning [`schema`].
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function schema_packed(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.schema_packed(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * The schema summary (classes + relations) read from the schema pyramid over
 * HTTP range — a Schema view of a remote graph without downloading it.
 * Worker-only (synchronous XHR). JSON:
 * `{ "kind":"schema", "classes":[[iri,count]], "relations":[[s,p,o,count]], "remote":{…} }`.
 * @param {string} url
 * @returns {string}
 */
export function schema_url(url) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.schema_url(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Validate a `.rete` graph against SHACL Core shapes written in Turtle.
 *
 * The default graph is validated unless `graph` names a dataset graph IRI.
 * `format` is one of:
 * - `"json"`: structured validation report from rete-core
 * - `"ttl"`: Turtle validation report
 * - anything else: compact text report
 *
 * A non-conformant graph returns a report; it is not a JS exception. Exceptions
 * are reserved for parse/open errors.
 * @param {Uint8Array} bytes
 * @param {string} shapes_turtle
 * @param {string | null | undefined} graph
 * @param {string} format
 * @returns {string}
 */
export function shacl(bytes, shapes_turtle, graph, format) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(shapes_turtle, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.shacl(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

/**
 * **Chain a SPARQL subset, then SHACL over it.** Evaluates a `CONSTRUCT` over
 * the remote `.rete` (touching only the tiles its patterns need), then validates
 * the resulting subgraph against the Turtle `shapes`. Where [`shacl_url`] selects
 * by *shape target* (validate every Person…), this selects by an explicit
 * `CONSTRUCT` — "validate just the slice this query carves out, in place".
 * Worker-only (synchronous XHR).
 * @param {string} url
 * @param {string} construct
 * @param {string} shapes_turtle
 * @param {string} format
 * @returns {string}
 */
export function shacl_construct_url(url, construct, shapes_turtle, format) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(construct, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(shapes_turtle, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.shacl_construct_url(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

/**
 * Validate a **remote** `.rete` graph (lazy HTTP range reads) against SHACL
 * Core shapes written in Turtle. Validating the **default** graph routes every
 * lookup as a range read ([`ReteGraph`]), so a targeted shape faults only the
 * tiles holding its targets — not the whole graph. A named graph (`graph`) still
 * materializes (the routed view is default-graph only). Worker-only (sync XHR).
 * @param {string} url
 * @param {string} shapes_turtle
 * @param {string | null | undefined} graph
 * @param {string} format
 * @returns {string}
 */
export function shacl_url(url, shapes_turtle, graph, format) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(shapes_turtle, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ptr3 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len3 = WASM_VECTOR_LEN;
        const ret = wasm.shacl_url(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

/**
 * @param {string} url
 * @param {string} query
 * @param {string} format
 * @returns {string}
 */
export function sparql_url(url, query, format) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.sparql_url(ptr0, len0, ptr1, len1, ptr2, len2);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Build the coarse-graph overview from a buffer in which only the header,
 * dictionary, and pyramid-summary ranges are populated — the index region may be
 * absent (zero-filled), because the summary path provably never reads it (see
 * the `ranged` test in rete-core). Returns JSON:
 * `{ "round", "communities", "predicateTotals": [["<iri>", count], ...] }`.
 * This is the "overview first, drill down later" payload, fetched in ~3 ranges.
 * @param {Uint8Array} bytes
 * @returns {string}
 */
export function summary_overview(bytes) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.summary_overview(ptr0, len0);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Full-text (word/CONTAINS) search over an embedded `.rete` image: the subjects
 * whose literals contain **every** word in `words` (whole-word, case-insensitive
 * — AND), optionally also a word starting with `contains_prefix`, as
 * `[{"subject":…}]`, capped at `limit`. Answers from the TEXT_INDEX section.
 * Empty array when the file has none (`build --text-index`).
 * @param {Uint8Array} bytes
 * @param {string[]} words
 * @param {string | null | undefined} contains_prefix
 * @param {number} limit
 * @returns {string}
 */
export function text_search(bytes, words, contains_prefix, limit) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArrayJsValueToWasm0(words, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(contains_prefix) ? 0 : passStringToWasm0(contains_prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        const ret = wasm.text_search(ptr0, len0, ptr1, len1, ptr2, len2, limit);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Check a curated card document without building anything — so an editor can
 * report the **exact** error `rete build --card-file` would report, while the
 * author is still typing. Returns the empty string when the document is
 * valid, otherwise the error message.
 *
 * Deliberately not a boolean: the wording is the useful part (a free-text
 * `theme` is told to use `keywords`; a stray top-level key is told about the
 * `extra` bag), and duplicating that wording in JavaScript is exactly how the
 * two writers would drift apart again.
 * @param {string} card_json
 * @returns {string}
 */
export function validate_card(card_json) {
    let deferred2_0;
    let deferred2_1;
    try {
        const ptr0 = passStringToWasm0(card_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.validate_card(ptr0, len0);
        deferred2_0 = ret[0];
        deferred2_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
    }
}

/**
 * Explain why each triple-pattern match is present in the `.rete` file.
 *
 * `null`/`undefined` positions are wildcards. The JSON uses browser-facing
 * camelCase fields:
 * `{ "pattern", "resultCount", "results": [{ "terms", "ids", "provenance" }] }`.
 * @param {Uint8Array} bytes
 * @param {string | null} [subject]
 * @param {string | null} [predicate]
 * @param {string | null} [object]
 * @returns {string}
 */
export function why_triples(bytes, subject, predicate, object) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(subject) ? 0 : passStringToWasm0(subject, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate) ? 0 : passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        var ptr3 = isLikeNone(object) ? 0 : passStringToWasm0(object, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len3 = WASM_VECTOR_LEN;
        const ret = wasm.why_triples(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}

/**
 * Triple-pattern provenance over a **remote** `.rete` URL (lazy range reads):
 * which permutation/section/byte-ranges answer the pattern. Worker-only.
 * @param {string} url
 * @param {string | null} [subject]
 * @param {string | null} [predicate]
 * @param {string | null} [object]
 * @returns {string}
 */
export function why_url(url, subject, predicate, object) {
    let deferred6_0;
    let deferred6_1;
    try {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(subject) ? 0 : passStringToWasm0(subject, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        var ptr2 = isLikeNone(predicate) ? 0 : passStringToWasm0(predicate, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len2 = WASM_VECTOR_LEN;
        var ptr3 = isLikeNone(object) ? 0 : passStringToWasm0(object, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len3 = WASM_VECTOR_LEN;
        const ret = wasm.why_url(ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
        var ptr5 = ret[0];
        var len5 = ret[1];
        if (ret[3]) {
            ptr5 = 0; len5 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred6_0 = ptr5;
        deferred6_1 = len5;
        return getStringFromWasm0(ptr5, len5);
    } finally {
        wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
    }
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_debug_string_0accd80f45e5faa2: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_is_function_754e9f305ff6029e: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_87c3bfe968c6a5ad: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_56732c2bc353f41d: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_c236cabd84a4d769: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_67b456be8673d3d7: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_string_get_72bdf95d3ae505b1: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_1506f2235d1bdba0: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_call_6e37a87ff352da3d: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.call(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_call_9c758de292015997: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_crypto_38df2bab126b63dc: function(arg0) {
            const ret = arg0.crypto;
            return ret;
        },
        __wbg_encodeURIComponent_9ff907ad9d03c7bb: function(arg0, arg1) {
            const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_error_78ff5b3a29b770e0: function(arg0) {
            console.error(arg0);
        },
        __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
            arg0.getRandomValues(arg1);
        }, arguments); },
        __wbg_getResponseHeader_db1ae5b1693dd680: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg1.getResponseHeader(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_get_de6a0f7d4d18a304: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_length_4a591ecaa01354d9: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_msCrypto_bd5a034af96bcba6: function(arg0) {
            const ret = arg0.msCrypto;
            return ret;
        },
        __wbg_new_2ee370dca414d926: function() { return handleError(function () {
            const ret = new XMLHttpRequest();
            return ret;
        }, arguments); },
        __wbg_new_50bb5ebeecef71a8: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_578aeef4b6b94378: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_a1b9f645bba64f0f: function() { return handleError(function () {
            const ret = new FileReaderSync();
            return ret;
        }, arguments); },
        __wbg_new_d90091b82fdf5b91: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_with_length_36a4998e27b014c5: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_new_with_length_b4a87ccced374381: function(arg0) {
            const ret = new Float64Array(arg0 >>> 0);
            return ret;
        },
        __wbg_node_84ea875411254db1: function(arg0) {
            const ret = arg0.node;
            return ret;
        },
        __wbg_open_837bab9ccb9e06da: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), arg5 !== 0);
        }, arguments); },
        __wbg_process_44c7a14e11e9f69e: function(arg0) {
            const ret = arg0.process;
            return ret;
        },
        __wbg_prototypesetcall_3249fc62a0fafa30: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_a6822215aa43e71c: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
            arg0.randomFillSync(arg1);
        }, arguments); },
        __wbg_readAsArrayBuffer_f1b8da05559618d9: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.readAsArrayBuffer(arg1);
            return ret;
        }, arguments); },
        __wbg_require_b4edbdcf3e2a1ef0: function() { return handleError(function () {
            const ret = module.require;
            return ret;
        }, arguments); },
        __wbg_responseText_266ec252b6be1e56: function() { return handleError(function (arg0, arg1) {
            const ret = arg1.responseText;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments); },
        __wbg_response_8ec82c168e320475: function() { return handleError(function (arg0) {
            const ret = arg0.response;
            return ret;
        }, arguments); },
        __wbg_send_4e22a258e556a44c: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.send(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
        }, arguments); },
        __wbg_send_dce79f146638dfda: function() { return handleError(function (arg0) {
            arg0.send();
        }, arguments); },
        __wbg_setRequestHeader_b5e8e6d03614f3e5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setRequestHeader(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_set_index_c69336ea758c0507: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_responseType_cfb49ea8269f8317: function(arg0, arg1) {
            arg0.responseType = __wbindgen_enum_XmlHttpRequestResponseType[arg1];
        },
        __wbg_size_9970092b88b1094c: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_02bb778501725738: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_9d53f2689e622ca1: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_a1a35cec07001a8a: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_4c59f6c7ea29a144: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_e70ae9f2eb052253: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_214edd0820ca76fc: function() { return handleError(function (arg0) {
            const ret = arg0.status;
            return ret;
        }, arguments); },
        __wbg_subarray_4aa221f6a4f5ab22: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_versions_276b2795b1c6a219: function(arg0) {
            const ret = arg0.versions;
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./rete_wasm_bg.js": import0,
    };
}

const __wbindgen_enum_XmlHttpRequestResponseType = ["", "arraybuffer", "blob", "document", "json", "text"];
const GraphFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_graph_free(ptr, 1));
const QuadCursorFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_quadcursor_free(ptr, 1));
const RemoteGraphFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_remotegraph_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('rete_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
