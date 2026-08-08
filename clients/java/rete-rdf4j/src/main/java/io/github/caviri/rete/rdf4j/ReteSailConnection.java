package io.github.caviri.rete.rdf4j;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.github.caviri.rete.QuadCursor;
import io.github.caviri.rete.Rete;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Iterator;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.function.Supplier;
import org.eclipse.rdf4j.common.iteration.CloseableIteration;
import org.eclipse.rdf4j.common.iteration.CloseableIteratorIteration;
import org.eclipse.rdf4j.model.IRI;
import org.eclipse.rdf4j.model.Namespace;
import org.eclipse.rdf4j.model.Resource;
import org.eclipse.rdf4j.model.Statement;
import org.eclipse.rdf4j.model.Value;
import org.eclipse.rdf4j.model.ValueFactory;
import org.eclipse.rdf4j.query.BindingSet;
import org.eclipse.rdf4j.query.Dataset;
import org.eclipse.rdf4j.query.QueryEvaluationException;
import org.eclipse.rdf4j.query.algebra.TupleExpr;
import org.eclipse.rdf4j.query.algebra.evaluation.EvaluationStrategy;
import org.eclipse.rdf4j.query.algebra.evaluation.QueryEvaluationStep;
import org.eclipse.rdf4j.query.algebra.evaluation.TripleSource;
import org.eclipse.rdf4j.query.algebra.evaluation.impl.DefaultEvaluationStrategy;
import org.eclipse.rdf4j.rio.helpers.NTriplesUtil;
import org.eclipse.rdf4j.sail.SailException;
import org.eclipse.rdf4j.sail.SailReadOnlyException;
import org.eclipse.rdf4j.sail.helpers.AbstractSailConnection;

/**
 * The {@link org.eclipse.rdf4j.sail.SailConnection} for {@link ReteSail}. Two
 * methods carry the weight: {@link #getStatementsInternal} maps a triple
 * pattern to a rete scan, and {@link #evaluateInternal} hands the query algebra
 * to RDF4J's {@link DefaultEvaluationStrategy} over a {@link TripleSource} that
 * is itself backed by that scan. Everything else is a read-only throw or a
 * no-op. rete term strings are N-Triples syntax, so {@link NTriplesUtil} maps
 * both directions.
 */
class ReteSailConnection extends AbstractSailConnection {

    private static final ObjectMapper JSON = new ObjectMapper();

    private final boolean ranged;
    private final byte[] image; // null when ranged
    private final ValueFactory vf;
    private final Rete engine;

    ReteSailConnection(ReteSail sail) {
        super(sail);
        this.vf = sail.getValueFactory();
        this.ranged = sail.isRanged();
        this.image = ranged ? null : sail.image();
        this.engine = sail.openEngine();
    }

    // Image vs ranged dispatch — the rest of the connection is source-agnostic.

    private java.util.List<String[]> scanQuads(String s, String p, String o) {
        return ranged ? engine.scanQuads(s, p, o) : engine.scanQuads(image, s, p, o);
    }

    private java.util.List<String[]> scanInGraph(String graph, String s, String p, String o) {
        return ranged
                ? engine.scanInGraph(graph, s, p, o)
                : engine.scanInGraph(image, graph, s, p, o);
    }

    private List<String> graphList() {
        return ranged ? engine.graphs() : engine.graphs(image);
    }

    private String infoJson() {
        return ranged ? engine.info() : engine.info(image);
    }

    // --- the two load-bearing methods -------------------------------------

    @Override
    protected CloseableIteration<? extends Statement> getStatementsInternal(
            Resource subj, IRI pred, Value obj, boolean includeInferred, Resource... contexts) {
        return cursor(subj, pred, obj, contexts);
    }

    @Override
    protected CloseableIteration<? extends BindingSet> evaluateInternal(
            TupleExpr tupleExpr, Dataset dataset, BindingSet bindings, boolean includeInferred) {
        TripleSource tripleSource = new ReteTripleSource();
        EvaluationStrategy strategy = new DefaultEvaluationStrategy(tripleSource, dataset, null);
        try {
            QueryEvaluationStep step = strategy.precompile(tupleExpr.clone());
            return step.evaluate(bindings);
        } catch (QueryEvaluationException e) {
            throw new SailException(e);
        }
    }

    /**
     * Scan a triple pattern, honouring RDF4J's context semantics: <b>no
     * contexts</b> means every graph (the union — each statement carries its own
     * graph), a <b>null</b> context is the default graph, and a <b>Resource</b>
     * context is that named graph. Bound subject/predicate/object are rendered to
     * N-Triples for the engine; graph IRIs are the plain string rete stores.
     *
     * <p>Returns a cursor, not a list, on <b>both</b> sides of the boundary. A
     * match becomes a {@link Statement} only when the consumer asks for it; and
     * over a lazily opened file the engine itself streams, pulling a bounded
     * batch of rows per wasm call instead of building the whole result first.
     * That is what makes {@code SELECT ?s ?p ?o … LIMIT 1} answerable: RDF4J
     * issues exactly {@code getStatements(null, null, null)} for it — the
     * {@code LIMIT} sits above the triple source, so the Sail never sees it —
     * takes one row, and closes the iteration. Materializing the scan first
     * turned that query into a whole-graph read.
     *
     * <p>When several contexts are named, a context is scanned only once the
     * previous one is exhausted. The in-memory ({@code byte[]}) path still
     * buffers: the image is already resident in linear memory there, so a cursor
     * would bound nothing.
     */
    private CloseableIteration<Statement> cursor(
            Resource subj, IRI pred, Value obj, Resource... contexts) {
        String s = subj == null ? null : NTriplesUtil.toNTriplesString(subj);
        String p = pred == null ? null : NTriplesUtil.toNTriplesString(pred);
        String o = obj == null ? null : NTriplesUtil.toNTriplesString(obj);
        List<String> graphs = null;
        if (contexts != null && contexts.length > 0) {
            // Graph identifiers are N-Triples terms (as rete stores them), the
            // same encoding as s/p/o — not a plain IRI string.
            graphs = new ArrayList<>(contexts.length);
            for (Resource ctx : contexts) {
                graphs.add(ctx == null ? null : NTriplesUtil.toNTriplesString(ctx));
            }
        }
        return ranged
                ? new StreamingCursor(s, p, o, graphs)
                : new BufferedCursor(s, p, o, graphs);
    }

    /**
     * The streaming cursor over a lazily opened file: one engine-side
     * {@link QuadCursor} at a time, one {@code Statement} at a time. Its
     * {@link #close()} releases the engine-side cursor — on normal completion,
     * on early abandonment (RDF4J closes every iteration it stops pulling from),
     * and on an exception.
     */
    private final class StreamingCursor implements CloseableIteration<Statement> {
        private final String s;
        private final String p;
        private final String o;
        private final Iterator<String> graphs; // null = all-graphs quad scan
        private QuadCursor rows;
        private boolean closed;

        StreamingCursor(String s, String p, String o, List<String> graphs) {
            this.s = s;
            this.p = p;
            this.o = o;
            this.graphs = graphs == null ? null : graphs.iterator();
            if (graphs == null) {
                this.rows = guard(() -> engine.scanCursor(s, p, o));
            }
        }

        @Override
        public boolean hasNext() {
            if (closed) {
                return false;
            }
            while (rows == null || !rows.hasNext()) {
                if (rows != null) {
                    rows.close();
                    rows = null;
                }
                if (graphs == null || !graphs.hasNext()) {
                    return false;
                }
                String g = graphs.next();
                rows = guard(() -> engine.scanCursorInGraph(g, s, p, o));
            }
            return true;
        }

        @Override
        public Statement next() {
            if (!hasNext()) {
                throw new NoSuchElementException();
            }
            return guard(
                    () -> {
                        String[] r = rows.next();
                        return statement(r[0], r[1], r[2], r[3]);
                    });
        }

        @Override
        public void close() {
            closed = true;
            if (rows != null) {
                rows.close();
                rows = null;
            }
        }

        /** Any engine failure closes the cursor before it propagates. */
        private <T> T guard(Supplier<T> run) {
            try {
                return run.get();
            } catch (RuntimeException e) {
                close();
                throw e instanceof SailException se ? se : new SailException(e);
            }
        }
    }

    /**
     * The in-memory path: the scan is buffered, because the whole image already
     * is. Same statement-at-a-time behaviour on the JVM side.
     */
    private final class BufferedCursor implements CloseableIteration<Statement> {
        private final String s;
        private final String p;
        private final String o;
        private final Iterator<String> graphs; // null = all-graphs quad scan
        private Iterator<String[]> rows;
        private String graph;

        BufferedCursor(String s, String p, String o, List<String> graphs) {
            this.s = s;
            this.p = p;
            this.o = o;
            this.graphs = graphs == null ? null : graphs.iterator();
            this.rows = graphs == null ? scan(() -> scanQuads(s, p, o)) : Collections.emptyIterator();
        }

        @Override
        public boolean hasNext() {
            while (!rows.hasNext() && graphs != null && graphs.hasNext()) {
                graph = graphs.next();
                String g = graph;
                rows = scan(() -> scanInGraph(g, s, p, o));
            }
            return rows.hasNext();
        }

        @Override
        public Statement next() {
            if (!hasNext()) {
                throw new NoSuchElementException();
            }
            String[] r = rows.next();
            // Quad rows carry the graph in slot 3; graph-scoped rows are triples.
            return statement(r[0], r[1], r[2], graphs == null ? r[3] : graph);
        }

        @Override
        public void close() {
            rows = Collections.emptyIterator();
        }

        private Iterator<String[]> scan(Supplier<List<String[]>> run) {
            try {
                return run.get().iterator();
            } catch (RuntimeException e) {
                throw new SailException(e);
            }
        }
    }

    /**
     * Build a Statement from N-Triples terms; a null {@code graph} = default
     * graph. {@code graph} (when present) is itself an N-Triples term.
     */
    private Statement statement(String s, String p, String o, String graph) {
        Resource subj = (Resource) NTriplesUtil.parseValue(s, vf);
        IRI pred = (IRI) NTriplesUtil.parseValue(p, vf);
        Value obj = NTriplesUtil.parseValue(o, vf);
        if (graph == null) {
            return vf.createStatement(subj, pred, obj);
        }
        return vf.createStatement(subj, pred, obj, (Resource) NTriplesUtil.parseValue(graph, vf));
    }

    /** A rete-backed {@link TripleSource} for RDF4J's evaluation strategy. */
    private final class ReteTripleSource implements TripleSource {
        @Override
        public CloseableIteration<? extends Statement> getStatements(
                Resource subj, IRI pred, Value obj, Resource... contexts) {
            return cursor(subj, pred, obj, contexts);
        }

        @Override
        public ValueFactory getValueFactory() {
            return vf;
        }
    }

    // --- lifecycle / size --------------------------------------------------

    @Override
    protected void closeInternal() {
        engine.close();
    }

    @Override
    protected CloseableIteration<? extends Resource> getContextIDsInternal() {
        List<Resource> ids = new ArrayList<>();
        for (String graph : graphList()) {
            // Graph names come back as N-Triples terms (e.g. "<http://…>").
            ids.add((Resource) NTriplesUtil.parseValue(graph, vf));
        }
        return new CloseableIteratorIteration<>(ids.iterator());
    }

    @Override
    protected long sizeInternal(Resource... contexts) {
        // No contexts = every statement in the dataset, which the header already
        // knows. This used to materialize the entire graph — every quad through
        // the wasm boundary and into a List — to count it, so `size()` on a large
        // file was an OOM rather than a slow answer. Reading the header is two
        // range reads on a lazily opened file, whatever its size.
        if (contexts == null || contexts.length == 0) {
            return quadCount();
        }
        // Named contexts: still a scan, because the header counts the dataset,
        // not each graph. Over a lazily opened file it is a STREAMING scan —
        // counting a graph must not require holding it, which was the whole
        // failure mode this Sail had for `?s ?p ?o`.
        long total = 0;
        for (Resource ctx : contexts) {
            String graph = ctx == null ? null : NTriplesUtil.toNTriplesString(ctx);
            if (ranged) {
                try (QuadCursor rows = engine.scanCursorInGraph(graph, null, null, null)) {
                    while (rows.hasNext()) {
                        rows.next();
                        total++;
                    }
                }
            } else {
                total += scanInGraph(graph, null, null, null).size();
            }
        }
        return total;
    }

    /** The dataset's total quad count, from the {@code .rete} header summary. */
    private long quadCount() {
        String json = infoJson();
        try {
            return JSON.readTree(json).get("quads").asLong();
        } catch (JsonProcessingException | RuntimeException e) {
            throw new SailException("could not read the quad count from the header: " + json, e);
        }
    }

    // --- transactions: no-ops (read-only, no write set) --------------------

    @Override
    protected void startTransactionInternal() {
        // no-op
    }

    @Override
    protected void commitInternal() {
        // no-op
    }

    @Override
    protected void rollbackInternal() {
        // no-op
    }

    // --- writes: rejected (read-only) --------------------------------------

    @Override
    protected void addStatementInternal(Resource subj, IRI pred, Value obj, Resource... contexts)
            throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }

    @Override
    protected void removeStatementsInternal(Resource subj, IRI pred, Value obj, Resource... contexts)
            throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }

    @Override
    protected void clearInternal(Resource... contexts) throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }

    // --- namespaces: none, and read-only ----------------------------------

    @Override
    protected CloseableIteration<? extends Namespace> getNamespacesInternal() {
        return new CloseableIteratorIteration<>(Collections.<Namespace>emptyList().iterator());
    }

    @Override
    protected String getNamespaceInternal(String prefix) {
        return null;
    }

    @Override
    protected void setNamespaceInternal(String prefix, String name) throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }

    @Override
    protected void removeNamespaceInternal(String prefix) throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }

    @Override
    protected void clearNamespacesInternal() throws SailException {
        throw new SailReadOnlyException("rete Sail is read-only");
    }
}
