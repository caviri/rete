let wasm_bindgen = (function(exports) {
    let script_src;
    if (typeof document !== 'undefined' && document.currentScript !== null) {
        script_src = new URL(document.currentScript.src, location.href).toString();
    }

    /**
     * A `.rete` opened **once** and kept resident, so a client (the playground's
     * cached/in-memory mode) can run many queries on a big file without re-copying
     * the whole buffer into wasm and re-decoding its dictionary on every call. The
     * methods mirror the free functions above but operate on the already-open
     * [`Rete`]. The few index-free readers (`schema_packed`, `progressive_query`,
     * `check_schema`) stay free functions — they read small ranges from the buffer
     * and are called rarely (once at load / on demand), so a handle buys little.
     */
    class Graph {
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                throw takeObject(ret[1]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
                }
                deferred2_0 = ptr1;
                deferred2_1 = len1;
                return getStringFromWasm0(ptr1, len1);
            } finally {
                wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
            }
        }
        /**
         * A **lazy, resumable cursor** over every quad of this graph — the
         * streaming export path. See [`QuadCursor`]; `graph` selects one graph
         * (`""` = the default graph), `None` streams the default graph followed by
         * every named graph.
         * @param {string | null} [graph]
         * @returns {QuadCursor}
         */
        quads(graph) {
            var ptr0 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            const ret = wasm.graph_quads(this.__wbg_ptr, ptr0, len0);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
                }
                deferred3_0 = ptr2;
                deferred3_1 = len2;
                return getStringFromWasm0(ptr2, len2);
            } finally {
                wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
                }
                deferred5_0 = ptr4;
                deferred5_1 = len4;
                return getStringFromWasm0(ptr4, len4);
            } finally {
                wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
            }
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
                    throw takeObject(ret[2]);
                }
                deferred4_0 = ptr3;
                deferred4_1 = len3;
                return getStringFromWasm0(ptr3, len3);
            } finally {
                wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
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
                    throw takeObject(ret[2]);
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
    exports.Graph = Graph;

    /**
     * A **lazy, resumable cursor** over every quad of an open `.rete` — the engine
     * side of `for await (const [s, p, o, g] of graph.quads())` in the JS client.
     *
     * # Why a cursor and not a callback
     *
     * [`Rete::dump_each`] already streams in constant memory, but a Rust callback
     * cannot be *paused* to hand control back to JavaScript: to feed a JS iterator
     * it would have to buffer every quad first, which is exactly the `Vec` that
     * [`Rete::dump`] builds and that OOMs on a large file. This wraps
     * [`Rete::dump_iter`] instead, so the scan can be suspended between calls and
     * resumed in place — one triple decoded per `next()`, never a whole-graph
     * materialization anywhere in the pipeline.
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
     * The dictionary is prefetched once (a dump resolves every term anyway), and
     * index tiles fault in as the scan advances and stay resident, so a full dump
     * of a *remote* graph ends up fetching essentially the whole file. Peak memory
     * is O(dictionary + index), never O(quads).
     */
    class QuadCursor {
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
                throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
    exports.QuadCursor = QuadCursor;

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
    class RemoteGraph {
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
                throw takeObject(ret[2]);
            }
            let v1;
            if (ret[0] !== 0) {
                v1 = getStringFromWasm0(ret[0], ret[1]).slice();
                wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
            }
            return v1;
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
                    throw takeObject(ret[2]);
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
                throw takeObject(ret[1]);
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
                    throw takeObject(ret[2]);
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
         * remote handle. It streams and stays memory-bounded exactly as the local
         * one does, but it is not *network*-lazy: a full dump resolves every term
         * and visits every tile, so it ends up fetching essentially the whole file
         * (and the tiles it faults stay resident). Use it to export a remote graph,
         * not to peek at one — for that, run a `LIMIT` query. Worker-only in the
         * browser, like every other read here.
         * @param {string | null} [graph]
         * @returns {QuadCursor}
         */
        quads(graph) {
            var ptr0 = isLikeNone(graph) ? 0 : passStringToWasm0(graph, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            const ret = wasm.remotegraph_quads(this.__wbg_ptr, ptr0, len0);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
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
                    throw takeObject(ret[2]);
                }
                deferred5_0 = ptr4;
                deferred5_1 = len4;
                return getStringFromWasm0(ptr4, len4);
            } finally {
                wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
            }
        }
        /**
         * `{ fileLength, bytes, requests }` — CUMULATIVE physical fetches since this
         * session opened. The worker diffs successive calls to report a single
         * query's traffic (a fully cached re-run adds ~0).
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
                    throw takeObject(ret[2]);
                }
                deferred4_0 = ptr3;
                deferred4_1 = len3;
                return getStringFromWasm0(ptr3, len3);
            } finally {
                wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
            }
        }
    }
    if (Symbol.dispose) RemoteGraph.prototype[Symbol.dispose] = RemoteGraph.prototype.free;
    exports.RemoteGraph = RemoteGraph;

    /**
     * Module init: route Rust panics to `console.error` with their message and
     * location. In release wasm a panic otherwise aborts as a bare
     * `RuntimeError: unreachable` with no clue where — this turns that into a
     * `rete-wasm panic: panicked at '…', src/…:line` line in the devtools console,
     * so an intermittent first-query crash (e.g. a parser tripping on a flaky
     * range read) can actually be diagnosed.
     */
    function __start() {
        wasm.__start();
    }
    exports.__start = __start;

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
    function build(text, format) {
        const ptr0 = passStringToWasm0(text, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.build(ptr0, len0, ptr1, len1);
        if (ret[3]) {
            throw takeObject(ret[2]);
        }
        var v3 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v3;
    }
    exports.build = build;

    /**
     * The embedded **Dataset Card** — the file's own self-description (title,
     * description, license, provenance, counts, example queries) as the JSON text
     * it was written with, or `undefined` when the file carries none. Reads the
     * metadata section straight out of the buffer.
     * @param {Uint8Array} bytes
     * @returns {string | undefined}
     */
    function card(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.card(ptr0, len0);
        if (ret[3]) {
            throw takeObject(ret[2]);
        }
        let v2;
        if (ret[0] !== 0) {
            v2 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v2;
    }
    exports.card = card;

    /**
     * The embedded **Dataset Card of a remote `.rete`**, in **two small range
     * requests**: the header, then the metadata section it points at — never the
     * dictionary, index, or pyramid. This is the index-free CARD tier: a client
     * learns what a multi-gigabyte graph *is* for a few KB. `undefined` when the
     * file carries no card. Worker-only (synchronous XHR).
     * @param {string} url
     * @returns {string | undefined}
     */
    function card_url(url) {
        const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.card_url(ptr0, len0);
        if (ret[3]) {
            throw takeObject(ret[2]);
        }
        let v2;
        if (ret[0] !== 0) {
            v2 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v2;
    }
    exports.card_url = card_url;

    /**
     * **Index-free schema coherence (Tier-0)** over an in-memory `.rete`: read only
     * the header + pyramid-meta (never the dictionary or the triple index) and report
     * schema-level incoherent points (subClassOf cycles, unsatisfiable classes).
     * Errors if the file ships no schema pyramid.
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    function check_schema(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.check_schema = check_schema;

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
    function check_schema_url(url) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.check_schema_url = check_schema_url;

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
    function communities(bytes, round) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.communities = communities;

    /**
     * The file's byte layout, for the playground's byte-map view. JSON:
     * `{ "fileLength": N, "segments": [ { "kind", "label", "offset", "len" } ] }`
     * — segments sorted by offset; uncovered bytes are container framing.
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    function file_layout(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.file_layout = file_layout;

    /**
     * The named-graph IRIs of a dataset, as a JSON array.
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    function graph_names(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.graph_names = graph_names;

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
    function header_ranges(head) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.header_ranges = header_ranges;

    /**
     * The wasm linear memory's current size in bytes — the engine's high-water
     * mark, since wasm memory grows but never shrinks. Exposed so a host can
     * *measure* the streaming-dump memory claim instead of trusting it: sample it
     * before and after a full [`QuadCursor`] drain and the growth stays flat
     * however many quads went by, where materializing them all does not.
     * @returns {number}
     */
    function heap_bytes() {
        const ret = wasm.heap_bytes();
        return ret;
    }
    exports.heap_bytes = heap_bytes;

    /**
     * Header summary as JSON: `{ "quads": N, "terms": N, "pyramidLevels": N }`.
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    function info(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.info = info;

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
    function prefix_search(bytes, prefix, limit) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.prefix_search = prefix_search;

    /**
     * Answer a conservative subset of SPARQL exactly from the pyramid summary,
     * without opening the triple index. Unsupported query shapes return an error
     * instead of silently falling back to a full scan.
     * @param {Uint8Array} bytes
     * @param {string} query
     * @returns {string}
     */
    function progressive_query(bytes, query) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.progressive_query = progressive_query;

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
    function pyramid_tree(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.pyramid_tree = pyramid_tree;

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
    function query(bytes, query, format) {
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
                throw takeObject(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    exports.query = query;

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
    function query_communities(bytes, query, round) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.query_communities = query_communities;

    /**
     * Run a SPARQL SELECT; returns a JSON array of solution objects
     * (`{ "var": "value", ... }`).
     * @param {Uint8Array} bytes
     * @param {string} query
     * @returns {string}
     */
    function query_sparql(bytes, query) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.query_sparql = query_sparql;

    /**
     * Evaluate a triple pattern; `null`/`undefined` positions are wildcards.
     * Returns a JSON array of `[subject, predicate, object]` triples.
     * @param {Uint8Array} bytes
     * @param {string | null} [subject]
     * @param {string | null} [predicate]
     * @param {string | null} [object]
     * @returns {string}
     */
    function query_triples(bytes, subject, predicate, object) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.query_triples = query_triples;

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
    function reach(bytes, predicate, seeds, reverse) {
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
                throw takeObject(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    exports.reach = reach;

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
    function reach_url(url, predicate, seeds, reverse) {
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
                throw takeObject(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    exports.reach_url = reach_url;

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
    function reason(bytes, graph) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.reason = reason;

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
    function reason_construct_url(url, construct) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.reason_construct_url = reason_construct_url;

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
    function reason_url(url, graph) {
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
                throw takeObject(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    exports.reason_url = reason_url;

    /**
     * The ontology profile (the semantic coarse graph), as JSON:
     * `{ "classes": [["<iri>", count], ...],
     *    "relations": [["sClass","pred","oClass", count], ...] }`.
     * The "overview first" payload — render it before fetching any detail.
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    function schema(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.schema = schema;

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
    function schema_packed(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.schema_packed = schema_packed;

    /**
     * The schema summary (classes + relations) read from the schema pyramid over
     * HTTP range — a Schema view of a remote graph without downloading it.
     * Worker-only (synchronous XHR). JSON:
     * `{ "kind":"schema", "classes":[[iri,count]], "relations":[[s,p,o,count]], "remote":{…} }`.
     * @param {string} url
     * @returns {string}
     */
    function schema_url(url) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.schema_url = schema_url;

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
    function shacl(bytes, shapes_turtle, graph, format) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.shacl = shacl;

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
    function shacl_construct_url(url, construct, shapes_turtle, format) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.shacl_construct_url = shacl_construct_url;

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
    function shacl_url(url, shapes_turtle, graph, format) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.shacl_url = shacl_url;

    /**
     * @param {string} url
     * @param {string} query
     * @param {string} format
     * @returns {string}
     */
    function sparql_url(url, query, format) {
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
                throw takeObject(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    exports.sparql_url = sparql_url;

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
    function summary_overview(bytes) {
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
                throw takeObject(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    exports.summary_overview = summary_overview;

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
    function text_search(bytes, words, contains_prefix, limit) {
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
                throw takeObject(ret[2]);
            }
            deferred5_0 = ptr4;
            deferred5_1 = len4;
            return getStringFromWasm0(ptr4, len4);
        } finally {
            wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
        }
    }
    exports.text_search = text_search;

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
    function why_triples(bytes, subject, predicate, object) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.why_triples = why_triples;

    /**
     * Triple-pattern provenance over a **remote** `.rete` URL (lazy range reads):
     * which permutation/section/byte-ranges answer the pattern. Worker-only.
     * @param {string} url
     * @param {string | null} [subject]
     * @param {string | null} [predicate]
     * @param {string | null} [object]
     * @returns {string}
     */
    function why_url(url, subject, predicate, object) {
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
                throw takeObject(ret[2]);
            }
            deferred6_0 = ptr5;
            deferred6_1 = len5;
            return getStringFromWasm0(ptr5, len5);
        } finally {
            wasm.__wbindgen_free(deferred6_0, deferred6_1, 1);
        }
    }
    exports.why_url = why_url;
    
        // ---- injected: Asyncify env imports + driver (replaces require("env")) ----
        let __reteAD = 0, __retePending = null, __reteSleeping = false, __reteRes = 0;
        function __reteStack() {
          if (!__reteAD) {
            // While an unwind is in flight, a DRIVEN wasm-bindgen wrapper still
            // runs its epilogue on whatever the raw export returned — garbage —
            // and takeObject()/getStringFromWasm0()/__wbindgen_free() on garbage
            // corrupt the object heap and the allocator. That is the
            // "null function or function signature mismatch" family: every
            // suspend is one roll of the corruption dice, which is why multi-GB
            // files (dozens–hundreds of suspends per query) died where small
            // ones survived. Guard every public raw export: when a call ends in
            // the UNWINDING state, hand the wrapper a harmless [0,0,0,0] tuple
            // instead (ptr 0 / len 0 / no error — the exact shape wbindgen's own
            // throw path already frees safely); the drive loop discards that
            // pass's value and calls again after the rewind. `instance.exports`
            // is frozen, so rebind the closure's `wasm` to a patchable clone.
            wasm = Object.assign(Object.create(null), wasm);
            for (const k of Object.keys(wasm)) {
              if (typeof wasm[k] !== "function" || k.indexOf("__") === 0 || k.indexOf("asyncify_") === 0) continue;
              const orig = wasm[k];
              wasm[k] = function () {
                const r = orig.apply(null, arguments);
                return wasm.asyncify_get_state() === 1 ? [0, 0, 0, 0] : r;
              };
            }
            // The allocator IS asyncify-instrumented (it can reach panic_fmt),
            // so a wrapper re-marshaling its arguments while the instance is
            // REWINDING (state 2) would make malloc's prologue consume the
            // rewind buffer as if IT were being resumed. Pause the rewind
            // around allocator calls — at state 0 they run normally.
            for (const k of ["__wbindgen_malloc", "__wbindgen_realloc"]) {
              const orig = wasm[k];
              if (!orig) continue;
              wasm[k] = function () {
                if (wasm.asyncify_get_state() === 2) {
                  wasm.asyncify_stop_rewind();
                  const r = orig.apply(null, arguments);
                  wasm.asyncify_start_rewind(__reteAD);
                  return r;
                }
                return orig.apply(null, arguments);
              };
            }
            const SIZE = 16 << 20; // 16 MiB Asyncify stack — the engine's recursive eval is deep
            __reteAD = wasm.__wbindgen_malloc(8 + SIZE, 8);
            const d = new DataView(wasm.memory.buffer);
            d.setInt32(__reteAD, __reteAD + 8, true);
            d.setInt32(__reteAD + 4, __reteAD + 8 + SIZE, true);
          }
        }
        // wasm32 pointers arrive through `i32` imports, so anything the engine
        // allocates above 2 GiB reaches JS SIGN-EXTENDED — a negative number that
        // makes `mem.set(b, ptr)` throw `RangeError: offset is out of bounds`. The
        // heap really does cross 2 GiB on a big remote scan (measured at 2050 MB on
        // wikidata-1GB), and because wasm memory never shrinks, every later read in
        // that worker fails too. `>>> 0` restores the unsigned value; every pointer
        // crossing this boundary goes through it.
        function __reteP(ptr) { return ptr >>> 0; }
        function __reteStr(ptr, len) { const p = __reteP(ptr); return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer).slice(p, p + (len >>> 0))); }
        // cache:'no-store' is REQUIRED on WebKit (desktop Safari, and iOS when a user
        // forces concurrent reads): WebKit caches/coalesces concurrent same-URL Range
        // requests (this Promise.all fires many at once) and can hand back a
        // wrong-length or empty body → the engine decodes corrupt tiles → a wasm trap.
        // But no-store defeats the HTTP cache on EVERY read, so it needlessly taxes
        // Chromium/Firefox (the async default there) with cross-reload re-fetches —
        // and they handle concurrent ranges fine. So gate it to WebKit only.
        var __reteNoStore = (function () { try { var ua = (navigator.userAgent || "").toLowerCase(); return ua.indexOf("safari") >= 0 && ua.indexOf("chrome") < 0 && ua.indexOf("chromium") < 0 && ua.indexOf("android") < 0 && ua.indexOf("edg/") < 0; } catch (e) { return false; } })();
        async function __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) {
          const url = __reteStr(urlPtr, urlLen);
          const dv = new DataView(wasm.memory.buffer);
          const ranges = [];
          const offsB = __reteP(offsPtr), lensB = __reteP(lensPtr);
            for (let i = 0; i < n; i++) ranges.push([Number(dv.getBigUint64(offsB + i*8, true)), dv.getUint32(lensB + i*4, true)]);
          // Retry each range once after a short pause: this Promise.all fires a
          // BURST of concurrent fetches, and a single transient miss ("Failed to
          // fetch" on a flaky link, a 5xx blip) used to fail the whole query —
          // the sync XHR reader already retries, so async matches it.
          const one = ([o, l], attempt) =>
            fetch(url, { headers: { Range: 'bytes=' + o + '-' + (o+l-1) }, cache: __reteNoStore ? 'no-store' : 'default' })
              .then((r) => { if (r.status !== 206) throw new Error('Range status ' + r.status + ' (host must support HTTP range)'); return r.arrayBuffer(); })
              .then((b) => new Uint8Array(b))
              .catch((e) => {
                if (attempt >= 1) throw e;
                return new Promise((res) => setTimeout(res, 250)).then(() => one([o, l], attempt + 1));
              });
          const bufs = await Promise.all(ranges.map((r) => one(r, 0)));
          const mem = new Uint8Array(wasm.memory.buffer);
          let pos = __reteP(dstPtr), total = 0;
          // Each range MUST land at its fixed slot (cumulative REQUESTED length), and
          // its body MUST be exactly the requested length. A short/over response (the
          // symptom of the WebKit caching bug above) would otherwise misalign every
          // later range and crash the decoder with an inscrutable wasm trap — fail
          // loudly with a diagnosable error instead.
          for (let i = 0; i < bufs.length; i++) {
            const b = bufs[i], want = ranges[i][1];
            if (b.length !== want) throw new Error('Range length mismatch: got ' + b.length + ' of ' + want + ' bytes at offset ' + ranges[i][0] + ' (browser mishandled a concurrent HTTP Range request)');
            mem.set(b, pos); pos += want; total += want;
          }
          return total;
        }
        async function __reteDoLen(urlPtr, urlLen, outPtr) {
          const url = __reteStr(urlPtr, urlLen);
          // The FIRST cross-origin request to a cold object can transiently come back
          // with no readable length (CORS preflight, CDN cold-start) — which is why a
          // fresh load fails once ("could not determine length") then works on retry.
          // The sync reader already retries; do the same here (the asyncify path used
          // to give up after one attempt). HEAD first: Content-Length is the full size
          // and CORS-safelisted, so it is readable even when the host hides
          // Content-Range (e.g. Zenodo); fall back to a bytes=0-0 GET's Content-Range
          // for hosts that reject HEAD (HF signed storage). `!(total > 0)` also treats
          // a NaN (e.g. Content-Range "bytes 0-0/*") as "keep trying".
          let total = 0;
          for (let attempt = 0; attempt < 4 && !(total > 0); attempt++) {
            if (attempt) await new Promise((r) => setTimeout(r, 150 * attempt)); // 150, 300, 450 ms
            try { const h = await fetch(url, { method: 'HEAD' }); if (h.ok) total = Number(h.headers.get('content-length') || 0); } catch (e) { /* fall back */ }
            if (!(total > 0)) {
              try {
                const r = await fetch(url, { headers: { Range: 'bytes=0-0' } });
                const cr = r.headers.get('content-range');
                total = cr ? Number(cr.split('/')[1]) : Number(r.headers.get('content-length') || 0);
              } catch (e) { /* retry the whole probe */ }
            }
          }
          new DataView(wasm.memory.buffer).setBigUint64(__reteP(outPtr), BigInt(total > 0 ? total : 0), true);
          return total > 0 ? 1 : 0;
        }
        function __reteSuspend(makePromise) {
          if (!__reteSleeping) { __retePending = makePromise(); wasm.asyncify_start_unwind(__reteAD); __reteSleeping = true; return 0; }
          wasm.asyncify_stop_rewind(); __reteSleeping = false; return __reteRes;
        }
        function __reteFetchRanges(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr) { return __reteSuspend(() => __reteDoFetch(urlPtr, urlLen, offsPtr, lensPtr, n, dstPtr)); }
        function __reteFileLen(urlPtr, urlLen, outPtr) { return __reteSuspend(() => __reteDoLen(urlPtr, urlLen, outPtr)); }
        async function __reteDrive(thunk) {
          __reteStack();
          let r = thunk();
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            r = thunk();
          }
          return r;
        }
        // Asyncify allows exactly ONE suspended computation per instance: a
        // second entry while the first sleeps shares __reteAD/__reteSleeping and
        // both corrupt — the observed symptom was a fresh open whose length
        // probe was "answered" by a stale suspend in ~4 ms ("could not
        // determine length"). Serialize every driven entry through a promise
        // chain: cheap, and correct by construction.
        let __reteTurn = Promise.resolve();
        function __reteSerial(fn) {
          const run = __reteTurn.then(fn);
          __reteTurn = run.then(function () {}, function () {});
          return run;
        }
        exports.reteDrive = function (thunk) { return __reteSerial(function () { return __reteDrive(thunk); }); };
        exports.reteOpenRemote = function (url) { return __reteSerial(function () { return __reteOpenRemote(url); }); };
        // RAW-driven resident calls — the ROOT FIX for the "null function /
        // signature mismatch" family (proven in tests/gate/.cache/
        // asyncify_probe3.cjs: the wrapper-driven query traps at its first
        // suspend on a 17.5 GB file; the same query raw-driven completes in 12
        // suspend/rewind passes). A generated wasm-bindgen wrapper marshals its
        // arguments and unpacks its result tuple on EVERY drive pass; driving
        // the raw export instead marshals ONCE and touches the result only
        // after the rewind completes — exactly reteOpenRemote's shape.
        async function __reteCallRaw(call, unpackString) {
          __reteStack();
          let ret = call();
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            ret = call();
          }
          if (!unpackString) return ret;
          if (ret[3]) throw takeObject(ret[2]);
          try { return getStringFromWasm0(ret[0], ret[1]); }
          finally { wasm.__wbindgen_free(ret[0], ret[1], 1); }
        }
        exports.reteQueryRemote = function (g, query, format, reasoned) {
          return __reteSerial(function () {
            const ptr0 = passStringToWasm0(query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(format, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const raw = reasoned ? wasm.remotegraph_query_reasoned : wasm.remotegraph_query;
            return __reteCallRaw(function () { return raw(g.__wbg_ptr, ptr0, len0, ptr1, len1); }, true);
          });
        };
        exports.retePrefixSearchRemote = function (g, prefix, limit) {
          return __reteSerial(function () {
            const ptr0 = passStringToWasm0(prefix, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            return __reteCallRaw(function () { return wasm.remotegraph_prefix_search(g.__wbg_ptr, ptr0, len0, limit); }, true);
          });
        };
        // RAW-driven generic *_url call (schema_url, check_schema_url, shacl_url,
        // reach_url, why_url, …) — the worker's generic "call" path used to drive
        // the generated WRAPPER through suspend/rewind, which re-marshals its
        // arguments and runs its free()-epilogue on EVERY pass and trapped with
        // "null function or function signature mismatch" at the first suspend
        // (proven in tests/gate/.cache/schema_probe.cjs: wrapper-driven
        // schema_url traps; the same call raw-driven completes in 4 passes).
        // Every *_url export is string-in/string-out with the same multivalue
        // result tuple, so one marshaler covers them: a string becomes a
        // (ptr, len) pair, null/undefined an absent Option (0, 0), a boolean an
        // i32 — marshal ONCE, drive raw, unpack only after the rewind completes.
        exports.reteCallUrlRemote = function (fn) {
          const args = Array.prototype.slice.call(arguments, 1);
          return __reteSerial(function () {
            const raw = wasm[fn];
            if (typeof raw !== "function") return Promise.reject(new Error("no wasm export " + fn));
            const flat = [];
            for (let i = 0; i < args.length; i++) {
              const a = args[i];
              if (typeof a === "string") { flat.push(passStringToWasm0(a, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc), WASM_VECTOR_LEN); }
              else if (a === null || a === undefined) { flat.push(0, 0); }
              else if (typeof a === "boolean") { flat.push(a ? 1 : 0); }
              else { flat.push(a); }
            }
            return __reteCallRaw(function () { return raw.apply(null, flat); }, true);
          });
        };
        async function __reteOpenRemote(url) {
          __reteStack();
          const ptr0 = passStringToWasm0(url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
          const len0 = WASM_VECTOR_LEN;
          let ret = wasm.remotegraph_new(ptr0, len0);
          while (wasm.asyncify_get_state() === 1) {
            wasm.asyncify_stop_unwind();
            __reteRes = await __retePending;
            wasm.asyncify_start_rewind(__reteAD);
            ret = wasm.remotegraph_new(ptr0, len0);
          }
          if (ret[2]) throw takeObject(ret[1]);
          const g = Object.create(RemoteGraph.prototype);
          g.__wbg_ptr = ret[0];
          RemoteGraphFinalization.register(g, g.__wbg_ptr, g);
          return g;
        };
        const import1 = { rete_fetch_ranges: __reteFetchRanges, rete_file_len: __reteFileLen,
          // LEAF panic reporter (never in asyncify-imports): the wasm-side hook
          // passes the raw panic Location so a crash logs file:line without any
          // fmt machinery (formatting is instrumented — a panic while the
          // instance is unwinding/rewinding would recurse forever).
          rete_panic_report: function (p, l, line) {
            try { console.error("rete-wasm panic at " + (l ? __reteStr(p, l) : "(unknown)") + ":" + line); } catch (e) { /* ignore */ }
          } };

    const import2 = import1;
    const import3 = import1;

    function __wbg_get_imports() {
        const import0 = {
            __proto__: null,
            __wbg___wbindgen_is_function_754e9f305ff6029e: function(arg0) {
                const ret = typeof(getObject(arg0)) === 'function';
                return ret;
            },
            __wbg___wbindgen_is_object_56732c2bc353f41d: function(arg0) {
                const val = getObject(arg0);
                const ret = typeof(val) === 'object' && val !== null;
                return ret;
            },
            __wbg___wbindgen_is_string_c236cabd84a4d769: function(arg0) {
                const ret = typeof(getObject(arg0)) === 'string';
                return ret;
            },
            __wbg___wbindgen_is_undefined_67b456be8673d3d7: function(arg0) {
                const ret = getObject(arg0) === undefined;
                return ret;
            },
            __wbg___wbindgen_string_get_72bdf95d3ae505b1: function(arg0, arg1) {
                const obj = getObject(arg1);
                const ret = typeof(obj) === 'string' ? obj : undefined;
                var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                var len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            },
            __wbg___wbindgen_throw_1506f2235d1bdba0: function(arg0, arg1) {
                throw new Error(getStringFromWasm0(arg0, arg1));
            },
            __wbg_call_9c758de292015997: function() { return handleError(function (arg0, arg1, arg2) {
                const ret = getObject(arg0).call(getObject(arg1), getObject(arg2));
                return addHeapObject(ret);
            }, arguments); },
            __wbg_crypto_38df2bab126b63dc: function(arg0) {
                const ret = getObject(arg0).crypto;
                return addHeapObject(ret);
            },
            __wbg_encodeURIComponent_9ff907ad9d03c7bb: function(arg0, arg1) {
                const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
                return addHeapObject(ret);
            },
            __wbg_getRandomValues_c44a50d8cfdaebeb: function() { return handleError(function (arg0, arg1) {
                getObject(arg0).getRandomValues(getObject(arg1));
            }, arguments); },
            __wbg_get_de6a0f7d4d18a304: function() { return handleError(function (arg0, arg1) {
                const ret = Reflect.get(getObject(arg0), getObject(arg1));
                return addHeapObject(ret);
            }, arguments); },
            __wbg_length_4a591ecaa01354d9: function(arg0) {
                const ret = getObject(arg0).length;
                return ret;
            },
            __wbg_msCrypto_bd5a034af96bcba6: function(arg0) {
                const ret = getObject(arg0).msCrypto;
                return addHeapObject(ret);
            },
            __wbg_new_2ee370dca414d926: function() { return handleError(function () {
                const ret = new XMLHttpRequest();
                return addHeapObject(ret);
            }, arguments); },
            __wbg_new_50bb5ebeecef71a8: function(arg0, arg1) {
                const ret = new Error(getStringFromWasm0(arg0, arg1));
                return addHeapObject(ret);
            },
            __wbg_new_with_length_36a4998e27b014c5: function(arg0) {
                const ret = new Uint8Array(arg0 >>> 0);
                return addHeapObject(ret);
            },
            __wbg_node_84ea875411254db1: function(arg0) {
                const ret = getObject(arg0).node;
                return addHeapObject(ret);
            },
            __wbg_open_837bab9ccb9e06da: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
                getObject(arg0).open(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4), arg5 !== 0);
            }, arguments); },
            __wbg_process_44c7a14e11e9f69e: function(arg0) {
                const ret = getObject(arg0).process;
                return addHeapObject(ret);
            },
            __wbg_prototypesetcall_3249fc62a0fafa30: function(arg0, arg1, arg2) {
                Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), getObject(arg2));
            },
            __wbg_randomFillSync_6c25eac9869eb53c: function() { return handleError(function (arg0, arg1) {
                getObject(arg0).randomFillSync(takeObject(arg1));
            }, arguments); },
            __wbg_require_b4edbdcf3e2a1ef0: function() { return handleError(function () {
                const ret = module.require;
                return addHeapObject(ret);
            }, arguments); },
            __wbg_responseText_266ec252b6be1e56: function() { return handleError(function (arg0, arg1) {
                const ret = getObject(arg1).responseText;
                var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
                var len1 = WASM_VECTOR_LEN;
                getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
                getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
            }, arguments); },
            __wbg_send_4e22a258e556a44c: function() { return handleError(function (arg0, arg1, arg2) {
                getObject(arg0).send(arg1 === 0 ? undefined : getStringFromWasm0(arg1, arg2));
            }, arguments); },
            __wbg_setRequestHeader_b5e8e6d03614f3e5: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
                getObject(arg0).setRequestHeader(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
            }, arguments); },
            __wbg_static_accessor_GLOBAL_9d53f2689e622ca1: function() {
                const ret = typeof global === 'undefined' ? null : global;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            },
            __wbg_static_accessor_GLOBAL_THIS_a1a35cec07001a8a: function() {
                const ret = typeof globalThis === 'undefined' ? null : globalThis;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            },
            __wbg_static_accessor_SELF_4c59f6c7ea29a144: function() {
                const ret = typeof self === 'undefined' ? null : self;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            },
            __wbg_static_accessor_WINDOW_e70ae9f2eb052253: function() {
                const ret = typeof window === 'undefined' ? null : window;
                return isLikeNone(ret) ? 0 : addHeapObject(ret);
            },
            __wbg_status_214edd0820ca76fc: function() { return handleError(function (arg0) {
                const ret = getObject(arg0).status;
                return ret;
            }, arguments); },
            __wbg_subarray_4aa221f6a4f5ab22: function(arg0, arg1, arg2) {
                const ret = getObject(arg0).subarray(arg1 >>> 0, arg2 >>> 0);
                return addHeapObject(ret);
            },
            __wbg_versions_276b2795b1c6a219: function(arg0) {
                const ret = getObject(arg0).versions;
                return addHeapObject(ret);
            },
            __wbindgen_cast_0000000000000001: function(arg0) {
                // Cast intrinsic for `F64 -> Externref`.
                const ret = arg0;
                return addHeapObject(ret);
            },
            __wbindgen_cast_0000000000000002: function(arg0, arg1) {
                // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
                const ret = getArrayU8FromWasm0(arg0, arg1);
                return addHeapObject(ret);
            },
            __wbindgen_cast_0000000000000003: function(arg0, arg1) {
                // Cast intrinsic for `Ref(String) -> Externref`.
                const ret = getStringFromWasm0(arg0, arg1);
                return addHeapObject(ret);
            },
            __wbindgen_object_clone_ref: function(arg0) {
                const ret = getObject(arg0);
                return addHeapObject(ret);
            },
            __wbindgen_object_drop_ref: function(arg0) {
                takeObject(arg0);
            },
        };
        return {
            __proto__: null,
            "./rete_wasm_bg.js": import0,
            "env": import1,
            "env": import2,
            "env": import3,
        };
    }

    const GraphFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_graph_free(ptr, 1));
    const QuadCursorFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_quadcursor_free(ptr, 1));
    const RemoteGraphFinalization = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(ptr => wasm.__wbg_remotegraph_free(ptr, 1));

    function addHeapObject(obj) {
        if (heap_next === heap.length) heap.push(heap.length + 1);
        const idx = heap_next;
        heap_next = heap[idx];

        heap[idx] = obj;
        return idx;
    }

    function dropObject(idx) {
        if (idx < 1028) return;
        heap[idx] = heap_next;
        heap_next = idx;
    }

    function getArrayJsValueFromWasm0(ptr, len) {
        ptr = ptr >>> 0;
        const mem = getDataViewMemory0();
        const result = [];
        for (let i = ptr; i < ptr + 4 * len; i += 4) {
            result.push(takeObject(mem.getUint32(i, true)));
        }
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

    function getObject(idx) { return heap[idx]; }

    function handleError(f, args) {
        try {
            return f.apply(this, args);
        } catch (e) {
            wasm.__wbindgen_exn_store(addHeapObject(e));
        }
    }

    let heap = new Array(1024).fill(undefined);
    heap.push(undefined, null, true, false);

    let heap_next = heap.length;

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
        const mem = getDataViewMemory0();
        for (let i = 0; i < array.length; i++) {
            mem.setUint32(ptr + 4 * i, addHeapObject(array[i]), true);
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

    function takeObject(idx) {
        const ret = getObject(idx);
        dropObject(idx);
        return ret;
    }

    let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
    cachedTextDecoder.decode();
    function decodeText(ptr, len) {
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

        if (module_or_path === undefined && script_src !== undefined) {
            module_or_path = script_src.replace(/\.js$/, "_bg.wasm");
        }
        const imports = __wbg_get_imports();

        if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
            throw new Error('async variant: pass bytes, not a URL');
        }

        const { instance, module } = await __wbg_load(await module_or_path, imports);

        return __wbg_finalize_init(instance, module);
    }

    return Object.assign(__wbg_init, { initSync }, exports);
})({ __proto__: null });
