package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
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
import org.junit.jupiter.api.io.TempDir;

/**
 * The RDF4J Sail over a {@code .rete} <b>on disk</b>, read lazily by range:
 * {@link ReteSail#ReteSail(Path)}. The same assertions as
 * {@link ReteSailRemoteTest} makes over HTTP, because it is the same reader with
 * a {@code FileChannel} underneath — plus the one that only a file can make:
 * the Sail answers without reading the whole file.
 */
class ReteSailFileTest {

    private static final String NQ =
            "<http://ex/book1> <http://purl.org/dc/terms/title> \"Rete\" .\n"
                    + "<http://ex/book1> <http://ex/shelf> \"A1\" <http://ex/libA> .\n"
                    + "<http://ex/book2> <http://ex/shelf> \"B2\" <http://ex/libB> .\n";

    private static byte[] image;

    private Repository repo;

    @BeforeAll
    static void buildImage() {
        try (Rete rete = Rete.load()) {
            image = rete.build(NQ, "nq");
        }
    }

    @BeforeEach
    void setUp(@TempDir Path dir) throws IOException {
        Path file = dir.resolve("data.rete");
        Files.write(file, image);
        repo = new SailRepository(new ReteSail(file));
        repo.init();
    }

    @AfterEach
    void tearDown() {
        if (repo != null) {
            repo.shutDown();
        }
    }

    @Test
    void fileUnionQuery() {
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r =
                        conn.prepareTupleQuery("SELECT ?s ?p ?o WHERE { ?s ?p ?o }").evaluate()) {
            int rows = 0;
            while (r.hasNext()) {
                r.next();
                rows++;
            }
            assertEquals(3, rows, "default + both named graphs, read lazily from disk");
        }
    }

    @Test
    void fileGraphIriRestricts() {
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
    void fileContextIds() {
        try (RepositoryConnection conn = repo.getConnection();
                RepositoryResult<Resource> ids = conn.getContextIDs()) {
            Set<String> ctxs = new HashSet<>();
            ids.forEach(c -> ctxs.add(c.stringValue()));
            assertEquals(Set.of("http://ex/libA", "http://ex/libB"), ctxs);
        }
    }

    /** {@code size()} is the header's quad count now — same answer, no full scan. */
    @Test
    void fileSize() {
        try (RepositoryConnection conn = repo.getConnection()) {
            assertEquals(3L, conn.size());
        }
    }

    @Test
    void fileEqualsImage() {
        String query = "SELECT ?s WHERE { ?s <http://ex/shelf> ?o }";
        Set<String> fromFile = subjects(repo, query);

        Repository inMemory = new SailRepository(new ReteSail(image));
        inMemory.init();
        try {
            assertEquals(subjects(inMemory, query), fromFile);
            assertTrue(fromFile.contains("http://ex/book1"));
        } finally {
            inMemory.shutDown();
        }
    }

    /**
     * The point of the whole change: a bounded query must not read the file. A
     * three-triple fixture is far too small for the byte ratio to be dramatic, so
     * this asserts the mechanism rather than the ratio — that the engine reads
     * <em>ranges</em> at all, which is what removes the size ceiling.
     */
    @Test
    void aQueryReadsRangesNotTheWholeFile(@TempDir Path dir) throws IOException {
        // A file with a lot of statements, so a point query is a small fraction.
        StringBuilder nt = new StringBuilder();
        for (int i = 0; i < 20_000; i++) {
            nt.append("<http://ex/s").append(i).append("> <http://ex/p> <http://ex/o")
                    .append(i).append("> .\n");
        }
        byte[] big;
        try (Rete rete = Rete.load()) {
            big = rete.build(nt.toString(), "nt");
        }
        Path file = dir.resolve("big.rete");
        Files.write(file, big);

        try (Rete rete = Rete.openFile(file)) {
            String json = rete.query("SELECT ?o WHERE { <http://ex/s10000> <http://ex/p> ?o }");
            assertTrue(json.contains("http://ex/o10000"), json);
            assertTrue(
                    rete.bytesRead() < big.length,
                    "read " + rete.bytesRead() + " of " + big.length + " bytes");
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
