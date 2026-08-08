package io.github.caviri.rete.rdf4j;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
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
        return new CloseableIteratorIteration<>(cursor(subj, pred, obj, contexts));
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
     * <p>Returns a cursor, not a list: a match becomes a {@link Statement} only
     * when the consumer asks for it, and when several contexts are named, a
     * context is scanned only once the previous one is exhausted. So the
     * connection never holds every {@code Statement} of a scan at once — which
     * for a join over a large graph is the difference between a bounded heap and
     * an {@code OutOfMemoryError}. (The engine still materializes its own side of
     * one scan; a streaming cursor across the wasm boundary is issue #115's
     * remaining item.)
     */
    private Iterator<Statement> cursor(Resource subj, IRI pred, Value obj, Resource... contexts) {
        String s = subj == null ? null : NTriplesUtil.toNTriplesString(subj);
        String p = pred == null ? null : NTriplesUtil.toNTriplesString(pred);
        String o = obj == null ? null : NTriplesUtil.toNTriplesString(obj);
        if (contexts == null || contexts.length == 0) {
            return new StatementCursor(s, p, o, null);
        }
        // Graph identifiers are N-Triples terms (as rete stores them), the same
        // encoding as s/p/o — not a plain IRI string.
        List<String> graphs = new ArrayList<>(contexts.length);
        for (Resource ctx : contexts) {
            graphs.add(ctx == null ? null : NTriplesUtil.toNTriplesString(ctx));
        }
        return new StatementCursor(s, p, o, graphs);
    }

    /**
     * One scan at a time, one {@code Statement} at a time. With {@code graphs ==
     * null} it is the all-graphs quad scan and each row carries its own graph;
     * otherwise it walks the named contexts in order, scanning each lazily.
     */
    private final class StatementCursor implements Iterator<Statement> {
        private final String s;
        private final String p;
        private final String o;
        private final Iterator<String> graphs; // null = all-graphs quad scan
        private Iterator<String[]> rows;
        private String graph;

        StatementCursor(String s, String p, String o, List<String> graphs) {
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
            return new CloseableIteratorIteration<>(cursor(subj, pred, obj, contexts));
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
        // not each graph. Bounded by the size of the graphs actually asked for.
        long total = 0;
        for (Resource ctx : contexts) {
            String graph = ctx == null ? null : NTriplesUtil.toNTriplesString(ctx);
            total += scanInGraph(graph, null, null, null).size();
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
