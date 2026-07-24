package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.io.IOException;
import java.net.URI;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.eclipse.rdf4j.model.Resource;
import org.eclipse.rdf4j.query.TupleQueryResult;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.RepositoryConnection;
import org.eclipse.rdf4j.repository.RepositoryResult;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * The RDF4J Sail over a <b>remote</b> {@code .rete}: a {@link SailRepository}
 * backed by {@link ReteSail#ReteSail(URI)}, served by an in-process HTTP server
 * with {@code Range} support. Proves the full stack — RDF4J SPARQL, named
 * graphs, contexts — works when the file lives behind HTTP and is read lazily.
 */
class ReteSailRemoteTest {

    private static final String NQ =
            "<http://ex/book1> <http://purl.org/dc/terms/title> \"Rete\" .\n"
                    + "<http://ex/book1> <http://ex/shelf> \"A1\" <http://ex/libA> .\n"
                    + "<http://ex/book2> <http://ex/shelf> \"B2\" <http://ex/libB> .\n";

    private static byte[] image;

    private HttpRangeTestServer server;
    private Repository repo;

    @BeforeAll
    static void buildImage() {
        try (Rete rete = Rete.load()) {
            image = rete.build(NQ, "nq");
        }
    }

    @BeforeEach
    void setUp() throws IOException {
        server = new HttpRangeTestServer(image);
        repo = new SailRepository(new ReteSail(server.uri()));
        repo.init();
    }

    @AfterEach
    void tearDown() {
        if (repo != null) {
            repo.shutDown();
        }
        if (server != null) {
            server.close();
        }
    }

    @Test
    void remoteUnionQuery() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery("SELECT ?s ?p ?o WHERE { ?s ?p ?o }").evaluate()) {
            int rows = 0;
            while (r.hasNext()) {
                r.next();
                rows++;
            }
            assertEquals(3, rows, "default + both named graphs, fetched lazily over HTTP");
        }
    }

    @Test
    void remoteGraphIriRestricts() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery(
                                        "SELECT ?s WHERE { GRAPH <http://ex/libA> { ?s ?p ?o } }")
                                .evaluate()) {
            List<String> subjects = new java.util.ArrayList<>();
            while (r.hasNext()) {
                subjects.add(r.next().getValue("s").stringValue());
            }
            assertEquals(List.of("http://ex/book1"), subjects);
        }
    }

    @Test
    void remoteContextIds() {
        try (RepositoryConnection conn = repo.getConnection();
                RepositoryResult<Resource> ids = conn.getContextIDs()) {
            Set<String> ctxs = new HashSet<>();
            ids.forEach(c -> ctxs.add(c.stringValue()));
            assertEquals(Set.of("http://ex/libA", "http://ex/libB"), ctxs);
        }
    }

    @Test
    void remoteEqualsLocal() {
        // The remote answer must match a local SailRepository over the same image.
        String query = "SELECT ?s WHERE { ?s <http://ex/shelf> ?o }";
        Set<String> remote = subjects(repo, query);

        Repository local = new SailRepository(new ReteSail(image));
        local.init();
        try {
            assertEquals(subjects(local, query), remote);
            assertTrue(remote.contains("http://ex/book1"));
        } finally {
            local.shutDown();
        }
    }

    private static Set<String> subjects(Repository repository, String query) {
        Set<String> out = new HashSet<>();
        try (RepositoryConnection conn = repository.getConnection();
                TupleQueryResult r = conn.prepareTupleQuery(query).evaluate()) {
            while (r.hasNext()) {
                out.add(r.next().getValue("s").stringValue());
            }
        }
        return out;
    }
}
