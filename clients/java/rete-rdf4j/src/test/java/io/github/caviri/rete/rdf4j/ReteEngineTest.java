package io.github.caviri.rete.rdf4j;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.caviri.rete.Rete;
import java.io.IOException;
import java.net.URI;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.eclipse.rdf4j.query.BindingSet;
import org.eclipse.rdf4j.query.TupleQueryResult;
import org.eclipse.rdf4j.repository.Repository;
import org.eclipse.rdf4j.repository.RepositoryConnection;
import org.eclipse.rdf4j.repository.sail.SailRepository;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * The {@link ReteEngine} fast-path (rete's own planner) must agree, result for
 * result, with the {@link ReteSail} path (RDF4J's engine) — the differential
 * check that the alternative evaluation route is correct, not just faster. Runs
 * the comparison both in-memory and over HTTP.
 */
class ReteEngineTest {

    private static final String NT =
            "<http://ex/alice> <http://ex/knows> <http://ex/bob> .\n"
                    + "<http://ex/bob> <http://ex/knows> <http://ex/carol> .\n"
                    + "<http://ex/alice> <http://ex/name> \"Alice\" .\n"
                    + "<http://ex/bob> <http://ex/name> \"Bob\" .\n"
                    + "<http://ex/carol> <http://ex/name> \"Carol\" .\n";

    private static final String JOIN =
            "SELECT ?p ?fn WHERE { ?p <http://ex/knows> ?f . ?f <http://ex/name> ?fn }";

    private static byte[] image;

    @BeforeAll
    static void build() {
        try (Rete rete = Rete.load()) {
            image = rete.build(NT, "nt");
        }
    }

    @Test
    void selectAgreesWithSailLocally() {
        List<BindingSet> rows;
        try (ReteEngine engine = ReteEngine.open(image)) {
            rows = engine.select(JOIN);
        }
        // Semantic check (format-agnostic): alice→Bob, bob→Carol.
        assertEquals(2, rows.size());
        Set<String> friendNames = new HashSet<>();
        for (BindingSet bs : rows) {
            friendNames.add(bs.getValue("fn").stringValue());
        }
        assertEquals(Set.of("Bob", "Carol"), friendNames);

        // Differential check: identical to RDF4J's own engine over the same data.
        assertEquals(
                sailSelect(new SailRepository(new ReteSail(image)), JOIN), normalize(rows));
    }

    @Test
    void askAndConstructLocally() {
        try (ReteEngine engine = ReteEngine.open(image)) {
            assertTrue(engine.ask("ASK { <http://ex/alice> <http://ex/knows> <http://ex/bob> }"));
            assertFalse(engine.ask("ASK { <http://ex/carol> <http://ex/knows> ?x }"));
            assertEquals(
                    1,
                    engine.construct(
                                    "CONSTRUCT { ?p <http://ex/friend> ?f } WHERE {"
                                        + " BIND(<http://ex/alice> AS ?p)"
                                        + " <http://ex/alice> <http://ex/knows> ?f }")
                            .size());
        }
    }

    @Test
    void selectAgreesWithSailRemotely() throws IOException {
        try (HttpRangeTestServer server = new HttpRangeTestServer(image)) {
            URI url = server.uri();

            List<BindingSet> rows;
            try (ReteEngine engine = ReteEngine.openRemote(url)) {
                rows = engine.select(JOIN);
            }

            // Three-way: remote planner == remote Sail == local ground truth.
            List<String> ground = sailSelect(new SailRepository(new ReteSail(image)), JOIN);
            assertEquals(ground, normalize(rows));
            assertEquals(ground, sailSelect(new SailRepository(new ReteSail(url)), JOIN));
        }
    }

    /** Evaluate through a SailRepository and normalize like {@link #normalize}. */
    private static List<String> sailSelect(Repository repo, String query) {
        repo.init();
        try (RepositoryConnection conn = repo.getConnection();
                TupleQueryResult r = conn.prepareTupleQuery(query).evaluate()) {
            List<BindingSet> rows = new ArrayList<>();
            while (r.hasNext()) {
                rows.add(r.next());
            }
            return normalize(rows);
        } finally {
            repo.shutDown();
        }
    }

    /** Canonicalize a solution bag to a sorted list of "var=value;…" strings. */
    private static List<String> normalize(List<BindingSet> rows) {
        List<String> out = new ArrayList<>(rows.size());
        for (BindingSet bs : rows) {
            List<String> names = new ArrayList<>(bs.getBindingNames());
            Collections.sort(names);
            StringBuilder sb = new StringBuilder();
            for (String n : names) {
                if (sb.length() > 0) {
                    sb.append(';');
                }
                sb.append(n).append('=').append(bs.getValue(n));
            }
            out.add(sb.toString());
        }
        Collections.sort(out);
        return out;
    }
}
