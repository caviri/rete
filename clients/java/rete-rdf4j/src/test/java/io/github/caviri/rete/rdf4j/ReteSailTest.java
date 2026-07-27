package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.util.ArrayList;
import java.util.List;
import org.eclipse.rdf4j.model.Statement;
import org.eclipse.rdf4j.query.BindingSet;
import org.eclipse.rdf4j.query.TupleQueryResult;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.RepositoryConnection;
import org.eclipse.rdf4j.repository.RepositoryException;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Drives a {@code .rete} image through the full RDF4J stack: a
 * {@link SailRepository} over {@link ReteSail}, queried with SPARQL. RDF4J's own
 * engine does the join/filter/ASK work, calling rete only for pattern scans — so
 * a green run proves rete is a first-class RDF4J store.
 */
class ReteSailTest {

    private static final String NT =
            "<http://example.org/book1> <http://purl.org/dc/terms/title> \"Rete\" .\n"
                    + "<http://example.org/book1> <http://purl.org/dc/terms/creator> <http://example.org/alice> .\n"
                    + "<http://example.org/alice> <http://xmlns.com/foaf/0.1/name> \"Alice\" .\n";

    private Repository repo;

    @BeforeEach
    void setUp() {
        byte[] image;
        try (Rete rete = Rete.load()) {
            image = rete.build(NT, "nt");
        }
        repo = new SailRepository(new ReteSail(image));
        repo.init();
    }

    @AfterEach
    void tearDown() {
        if (repo != null) {
            repo.shutDown();
        }
    }

    @Test
    void selectThroughRdf4jEngine() {
        List<String> titles = new ArrayList<>();
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult result =
                        conn.prepareTupleQuery(
                                        "SELECT ?title WHERE {"
                                                + " ?s <http://purl.org/dc/terms/title> ?title }")
                                .evaluate()) {
            while (result.hasNext()) {
                BindingSet bs = result.next();
                titles.add(bs.getValue("title").stringValue());
            }
        }
        assertEquals(List.of("Rete"), titles);
    }

    @Test
    void joinAndAskThroughRdf4jEngine() {
        try (RepositoryConnection conn = repo.getConnection()) {
            try (TupleQueryResult result =
                    conn.prepareTupleQuery(
                                    "SELECT ?name WHERE {"
                                            + " <http://example.org/book1> <http://purl.org/dc/terms/creator> ?a ."
                                            + " ?a <http://xmlns.com/foaf/0.1/name> ?name }")
                            .evaluate()) {
                assertTrue(result.hasNext(), "join returned no rows");
                assertEquals("Alice", result.next().getValue("name").stringValue());
            }

            boolean ask =
                    conn.prepareBooleanQuery(
                                    "ASK { <http://example.org/book1>"
                                            + " <http://purl.org/dc/terms/creator> <http://example.org/alice> }")
                            .evaluate();
            assertTrue(ask, "ASK should be true");
        }
    }

    @Test
    void getStatementsAndSize() {
        try (RepositoryConnection conn = repo.getConnection()) {
            assertEquals(3L, conn.size());

            List<Statement> byCreator = new ArrayList<>();
            conn.getStatements(
                            null,
                            repo.getValueFactory().createIRI("http://purl.org/dc/terms/creator"),
                            null,
                            false)
                    .forEach(byCreator::add);
            assertEquals(1, byCreator.size());
            assertEquals("http://example.org/alice", byCreator.get(0).getObject().stringValue());
        }
    }

    @Test
    void writesAreRejected() {
        try (RepositoryConnection conn = repo.getConnection()) {
            // The Sail is read-only: an add must fail. RDF4J surfaces the Sail's
            // SailReadOnlyException wrapped as a RepositoryException.
            assertThrows(
                    RepositoryException.class,
                    () ->
                            conn.add(
                                    repo.getValueFactory().createIRI("http://example.org/x"),
                                    repo.getValueFactory().createIRI("http://example.org/p"),
                                    repo.getValueFactory().createLiteral("y")));
        }
    }
}
