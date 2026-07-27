package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.eclipse.rdf4j.model.IRI;
import org.eclipse.rdf4j.model.Resource;
import org.eclipse.rdf4j.model.Statement;
import org.eclipse.rdf4j.model.ValueFactory;
import org.eclipse.rdf4j.query.BindingSet;
import org.eclipse.rdf4j.query.TupleQueryResult;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.RepositoryConnection;
import org.eclipse.rdf4j.repository.RepositoryResult;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Named-graph coverage through the full RDF4J stack. The dataset has one
 * default-graph triple plus two named graphs; the tests exercise {@code GRAPH
 * <iri>}, {@code GRAPH ?g}, the default-union of a plain pattern, per-context
 * {@code getStatements}, and {@code getContextIDs} — all driven by RDF4J's own
 * engine over the rete Sail.
 */
class ReteSailNamedGraphTest {

    private static final String NQ =
            "<http://ex/book1> <http://ex/title> \"Rete\" .\n" // default graph
                    + "<http://ex/book1> <http://ex/shelf> \"A1\" <http://ex/libA> .\n" // libA
                    + "<http://ex/book2> <http://ex/shelf> \"B2\" <http://ex/libB> .\n"; // libB

    private Repository repo;
    private ValueFactory vf;
    private IRI libA;
    private IRI libB;

    @BeforeEach
    void setUp() {
        byte[] image;
        try (Rete rete = Rete.load()) {
            image = rete.build(NQ, "nq");
        }
        repo = new SailRepository(new ReteSail(image));
        repo.init();
        vf = repo.getValueFactory();
        libA = vf.createIRI("http://ex/libA");
        libB = vf.createIRI("http://ex/libB");
    }

    @AfterEach
    void tearDown() {
        if (repo != null) {
            repo.shutDown();
        }
    }

    @Test
    void contextIdsAreTheNamedGraphs() {
        try (RepositoryConnection conn = repo.getConnection();
                RepositoryResult<Resource> ids = conn.getContextIDs()) {
            Set<Resource> ctxs = new HashSet<>();
            ids.forEach(ctxs::add);
            assertEquals(Set.of(libA, libB), ctxs);
        }
    }

    @Test
    void plainPatternIsTheUnionOfAllGraphs() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery("SELECT ?s ?p ?o WHERE { ?s ?p ?o }").evaluate()) {
            int rows = 0;
            while (r.hasNext()) {
                r.next();
                rows++;
            }
            assertEquals(3, rows, "default + both named graphs");
        }
    }

    @Test
    void graphIriRestrictsToThatGraph() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery(
                                        "SELECT ?s WHERE { GRAPH <http://ex/libA> { ?s ?p ?o } }")
                                .evaluate()) {
            List<String> subjects = new ArrayList<>();
            while (r.hasNext()) {
                subjects.add(r.next().getValue("s").stringValue());
            }
            assertEquals(List.of("http://ex/book1"), subjects);
        }
    }

    @Test
    void graphVariableBindsToEachNamedGraph() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery(
                                        "SELECT ?g WHERE { GRAPH ?g { ?s <http://ex/shelf> ?o } }")
                                .evaluate()) {
            Set<String> graphs = new HashSet<>();
            while (r.hasNext()) {
                BindingSet bs = r.next();
                graphs.add(bs.getValue("g").stringValue());
            }
            // The shelf triples live in the two named graphs, not the default one.
            assertEquals(Set.of("http://ex/libA", "http://ex/libB"), graphs);
        }
    }

    @Test
    void getStatementsByContextAndTheirContextTag() {
        try (RepositoryConnection conn = repo.getConnection()) {
            List<Statement> inB = new ArrayList<>();
            conn.getStatements(null, null, null, false, libB).forEach(inB::add);
            assertEquals(1, inB.size());
            Statement st = inB.get(0);
            assertEquals("http://ex/book2", st.getSubject().stringValue());
            assertEquals(libB, st.getContext());
        }
    }

    @Test
    void defaultContextIsIsolated() {
        try (RepositoryConnection conn = repo.getConnection()) {
            // The null context is the default graph: only the title triple, and
            // its statement carries no context.
            List<Statement> def = new ArrayList<>();
            conn.getStatements(null, null, null, false, (Resource) null).forEach(def::add);
            assertEquals(1, def.size());
            assertEquals("http://ex/title", def.get(0).getPredicate().stringValue());
            assertTrue(def.get(0).getContext() == null, "default-graph statement has no context");

            // Whole store spans all three graphs.
            assertEquals(3L, conn.size());
        }
    }
}
