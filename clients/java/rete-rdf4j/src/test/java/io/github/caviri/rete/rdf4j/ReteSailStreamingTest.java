package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import org.eclipse.rdf4j.common.iteration.CloseableIteration;
import org.eclipse.rdf4j.model.Statement;
import org.eclipse.rdf4j.query.TupleQueryResult;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.RepositoryConnection;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The Sail's unbounded scan has to <b>stream</b>, because RDF4J asks for exactly
 * that shape to answer the most trivial exploratory query there is.
 *
 * <p>{@code SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1} reaches the Sail as
 * {@code getStatements(null, null, null)} — the {@code LIMIT} is a {@code Slice}
 * above the triple source, so the Sail never sees it — and RDF4J takes one row
 * and closes the iteration. If the engine builds the whole result first, that
 * query costs a whole-graph read. These tests pin the property that it does not:
 * the engine produces one batch to answer {@code LIMIT 1}, not the graph, and
 * every abandoned iteration releases its engine-side cursor.
 */
class ReteSailStreamingTest {

    private static final int N = 20_000;

    @TempDir static Path dir;

    private static Path file;

    private Repository repo;

    @BeforeAll
    static void buildImage() throws IOException {
        StringBuilder nt = new StringBuilder(N * 60);
        for (int i = 0; i < N; i++) {
            nt.append("<http://ex/s").append(i).append("> <http://ex/p").append(i % 3)
                    .append("> <http://ex/o").append(i).append("> .\n");
        }
        byte[] image;
        try (Rete rete = Rete.load()) {
            image = rete.build(nt.toString(), "nt");
        }
        file = dir.resolve("streaming.rete");
        Files.write(file, image);
    }

    @AfterEach
    void tearDown() {
        System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        if (repo != null) {
            repo.shutDown();
            repo = null;
        }
    }

    /**
     * A Sail that remembers the engine its connection opened, so a test can ask
     * the engine what it read and how many cursors it still holds. The Sail's
     * own API deliberately exposes neither.
     */
    private static final class ProbeSail extends ReteSail {
        private Rete engine;

        ProbeSail(Path path) {
            super(path);
        }

        @Override
        Rete openEngine() {
            engine = super.openEngine();
            return engine;
        }
    }

    private ProbeSail probe() {
        ProbeSail sail = new ProbeSail(file);
        repo = new SailRepository(sail);
        repo.init();
        return sail;
    }

    /**
     * The headline. {@code LIMIT 1} must not scan the graph: the engine may
     * produce one batch, not {@value #N} rows.
     */
    @Test
    void limitOneDoesNotScanTheGraph() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "64");
        ProbeSail sail = probe();
        try (RepositoryConnection conn = repo.getConnection()) {
            try (TupleQueryResult r =
                    conn.prepareTupleQuery("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1")
                            .evaluate()) {
                assertTrue(r.hasNext(), "LIMIT 1 returned nothing");
                assertEquals("http://ex/p0", r.next().getValue("p").stringValue());
            }
            assertEquals(
                    32,
                    sail.engine.rowsStreamed(),
                    "SELECT ?s ?p ?o LIMIT 1 made the engine produce "
                            + sail.engine.rowsStreamed() + " of " + N
                            + " rows — the Sail is still materializing");
            assertEquals(1, sail.engine.batchCalls());
        }
    }

    /** The same for the raw Sail primitive RDF4J calls underneath. */
    @Test
    void oneStatementDoesNotScanTheGraph() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "64");
        ProbeSail sail = probe();
        try (RepositoryConnection conn = repo.getConnection()) {
            try (CloseableIteration<? extends Statement> it =
                    conn.getStatements(null, null, null, false)) {
                assertTrue(it.hasNext());
                assertEquals("http://ex/s0", it.next().getSubject().stringValue());
            }
            assertEquals(32, sail.engine.rowsStreamed());
        }
    }

    /** Streaming may not change the answer: a full drain is still every statement. */
    @Test
    void fullDrainIsUnchanged() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "512");
        probe();
        try (RepositoryConnection conn = repo.getConnection()) {
            List<String> subjects = new ArrayList<>();
            try (CloseableIteration<? extends Statement> it =
                    conn.getStatements(null, null, null, false)) {
                while (it.hasNext()) {
                    subjects.add(it.next().getSubject().stringValue());
                }
            }
            assertEquals(N, subjects.size());
            assertTrue(subjects.contains("http://ex/s0"));
            assertTrue(subjects.contains("http://ex/s" + (N - 1)));
            assertEquals(N, conn.size());
        }
    }

    /**
     * ABANDONMENT through the RDF4J API: a caller that stops pulling and closes
     * the iteration — which is exactly what RDF4J's own {@code Slice} does for
     * {@code LIMIT} — must leave no engine-side cursor behind. One leaked cursor
     * per query is a leak in any long-lived {@code Sail}.
     */
    @Test
    void abandonedIterationsReleaseTheirCursor() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "8");
        ProbeSail sail = probe();
        try (RepositoryConnection conn = repo.getConnection()) {
            for (int i = 0; i < 25; i++) {
                try (CloseableIteration<? extends Statement> it =
                        conn.getStatements(null, null, null, false)) {
                    it.next(); // one row out of 20,000, then abandoned
                }
            }
            assertEquals(
                    0,
                    sail.engine.openCursorCount(),
                    "25 abandoned getStatements left engine-side cursors behind");
            for (int i = 0; i < 25; i++) {
                try (TupleQueryResult r =
                        conn.prepareTupleQuery("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1").evaluate()) {
                    r.next();
                }
            }
            assertEquals(
                    0, sail.engine.openCursorCount(), "SPARQL LIMIT leaked a cursor per query");
        }
    }

    /** Closing the iteration twice, or after draining it, must be quiet. */
    @Test
    void closingIsIdempotent() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "4096");
        ProbeSail sail = probe();
        try (RepositoryConnection conn = repo.getConnection()) {
            CloseableIteration<? extends Statement> it =
                    conn.getStatements(null, null, null, false);
            it.next();
            it.close();
            it.close();
            assertFalse(it.hasNext(), "a closed iteration must report exhaustion");
            assertEquals(0, sail.engine.openCursorCount());
        }
    }

    /** Named contexts stream one at a time, and still answer exactly. */
    @Test
    void namedContextsStream() throws IOException {
        byte[] quads;
        try (Rete rete = Rete.load()) {
            quads =
                    rete.build(
                            "<http://ex/a> <http://ex/p> \"x\" <http://ex/g1> .\n"
                                    + "<http://ex/b> <http://ex/p> \"y\" <http://ex/g2> .\n"
                                    + "<http://ex/c> <http://ex/p> \"z\" <http://ex/g2> .\n",
                            "nq");
        }
        Path quadFile = dir.resolve("quads.rete");
        Files.write(quadFile, quads);
        ProbeSail sail = new ProbeSail(quadFile);
        repo = new SailRepository(sail);
        repo.init();
        try (RepositoryConnection conn = repo.getConnection()) {
            var g2 = org.eclipse.rdf4j.model.impl.SimpleValueFactory.getInstance()
                    .createIRI("http://ex/g2");
            List<String> subjects = new ArrayList<>();
            try (CloseableIteration<? extends Statement> it =
                    conn.getStatements(null, null, null, false, g2)) {
                while (it.hasNext()) {
                    subjects.add(it.next().getSubject().stringValue());
                }
            }
            assertEquals(List.of("http://ex/b", "http://ex/c"), subjects);
            assertEquals(2L, conn.size(g2));
            assertEquals(0, sail.engine.openCursorCount());
        }
    }
}
