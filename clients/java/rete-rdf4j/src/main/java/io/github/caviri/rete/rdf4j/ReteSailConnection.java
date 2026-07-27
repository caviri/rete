package io.github.caviri.rete.rdf4j;

import io.github.caviri.rete.Rete;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
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

    private final boolean remote;
    private final byte[] image; // null when remote
    private final ValueFactory vf;
    private final Rete engine;

    ReteSailConnection(ReteSail sail) {
        super(sail);
        this.vf = sail.getValueFactory();
        this.remote = sail.isRemote();
        if (remote) {
            this.image = null;
            this.engine = Rete.openRemote(sail.url());
        } else {
            this.image = sail.image();
            this.engine = Rete.load();
        }
    }

    // Local vs remote dispatch — the rest of the connection is source-agnostic.

    private java.util.List<String[]> scanQuads(String s, String p, String o) {
        return remote ? engine.scanQuadsRemote(s, p, o) : engine.scanQuads(image, s, p, o);
    }

    private java.util.List<String[]> scanInGraph(String graph, String s, String p, String o) {
        return remote
                ? engine.scanInGraphRemote(graph, s, p, o)
                : engine.scanInGraph(image, graph, s, p, o);
    }

    private List<String> graphList() {
        return remote ? engine.graphsRemote() : engine.graphs(image);
    }

    // --- the two load-bearing methods -------------------------------------

    @Override
    protected CloseableIteration<? extends Statement> getStatementsInternal(
            Resource subj, IRI pred, Value obj, boolean includeInferred, Resource... contexts) {
        return new CloseableIteratorIteration<>(statements(subj, pred, obj, contexts).iterator());
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
     * Scan a triple pattern into RDF4J Statements, honouring RDF4J's context
     * semantics: <b>no contexts</b> means every graph (the union — each
     * statement carries its own graph), a <b>null</b> context is the default
     * graph, and a <b>Resource</b> context is that named graph. Bound
     * subject/predicate/object are rendered to N-Triples for the engine; graph
     * IRIs are the plain string rete stores.
     */
    private List<Statement> statements(Resource subj, IRI pred, Value obj, Resource... contexts) {
        String s = subj == null ? null : NTriplesUtil.toNTriplesString(subj);
        String p = pred == null ? null : NTriplesUtil.toNTriplesString(pred);
        String o = obj == null ? null : NTriplesUtil.toNTriplesString(obj);
        List<Statement> out = new ArrayList<>();
        try {
            if (contexts == null || contexts.length == 0) {
                for (String[] q : scanQuads(s, p, o)) {
                    out.add(statement(q[0], q[1], q[2], q[3]));
                }
            } else {
                for (Resource ctx : contexts) {
                    // Graph identifiers are N-Triples terms (as rete stores them),
                    // the same encoding as s/p/o — not a plain IRI string.
                    String graph = ctx == null ? null : NTriplesUtil.toNTriplesString(ctx);
                    for (String[] t : scanInGraph(graph, s, p, o)) {
                        out.add(statement(t[0], t[1], t[2], graph));
                    }
                }
            }
        } catch (RuntimeException e) {
            throw new SailException(e);
        }
        return out;
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
            return new CloseableIteratorIteration<>(statements(subj, pred, obj, contexts).iterator());
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
        // v1: counts via a full scan (cheap for embedded files; a header/index
        // count is a follow-up). No contexts = every graph; else the named ones.
        if (contexts == null || contexts.length == 0) {
            return scanQuads(null, null, null).size();
        }
        long total = 0;
        for (Resource ctx : contexts) {
            String graph = ctx == null ? null : NTriplesUtil.toNTriplesString(ctx);
            total += scanInGraph(graph, null, null, null).size();
        }
        return total;
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
