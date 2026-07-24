package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.URI;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

/**
 * A LIVE integration check against a real published dataset served over HTTP
 * {@code Range} from Cloudflare R2 — the production path, not a toy in-process
 * server. Skipped by default (it needs the network); enable with
 * {@code -Drete.live=true}. It proves the client opens and queries a real,
 * multi-megabyte remote {@code .rete} while fetching only a small fraction of it.
 */
class LiveRemoteTest {

    private static final URI URL =
            URI.create("https://data.graphplaza.com/aifdb-open/aifdb-open.rete");
    private static final long SIZE = 13_580_488L; // per web/datasets.lock.json

    @Test
    void opensAndQueriesARealRemoteDatasetLazily() {
        Assumptions.assumeTrue(
                Boolean.getBoolean("rete.live"), "live network test — enable with -Drete.live=true");

        try (Rete rete = Rete.openRemote(URL)) {
            String info = rete.infoRemote();
            assertTrue(info.contains("\"quads\":"), info);

            String rows = rete.queryRemote("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3");
            assertTrue(rows.contains("\"kind\":\"select\""), rows);

            long fetched = rete.bytesFetched();
            assertTrue(fetched > 0, "should fetch something");
            assertTrue(
                    fetched < SIZE, "expected a fraction of " + SIZE + " bytes, fetched " + fetched);

            System.out.printf(
                    "[live] opened + queried a %.1f MB remote .rete, fetching %.1f KB (%.2f%% of the file)%n",
                    SIZE / 1e6, fetched / 1e3, 100.0 * fetched / SIZE);
            System.out.println("[live] " + info);
        }
    }
}
