package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpHandler;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.URI;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicLong;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Exercises the <b>remote, lazy</b> path against a real in-process HTTP server
 * that supports {@code Range} requests. Proves two things: the engine returns
 * the same answers over HTTP as in-memory, and it fetches only a fraction of the
 * file (range reads, not a full download).
 */
class ReteRemoteTest {

    private static final int N = 20_000;

    // Built once: a dataset big enough that the index dwarfs any single point
    // query's fetch — so laziness is observable, not masked by a one-block file.
    private static byte[] image;

    private HttpServer server;
    private URI url;

    @BeforeAll
    static void buildImage() {
        StringBuilder nt = new StringBuilder(N * 60);
        for (int i = 0; i < N; i++) {
            nt.append("<http://ex/s").append(i).append("> <http://ex/p> <http://ex/o")
                    .append(i).append("> .\n");
        }
        try (Rete rete = Rete.load()) {
            image = rete.build(nt.toString(), "nt");
        }
    }

    @BeforeEach
    void setUp() throws IOException {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/data.rete", new RangeHandler(image));
        server.start();
        url = URI.create("http://127.0.0.1:" + server.getAddress().getPort() + "/data.rete");
    }

    @AfterEach
    void tearDown() {
        if (server != null) {
            server.stop(0);
        }
    }

    @Test
    void remoteInfoMatchesTheFile() {
        try (Rete rete = Rete.openRemote(url)) {
            assertTrue(rete.infoRemote().contains("\"quads\":" + N), rete.infoRemote());
        }
    }

    @Test
    void remoteQueryMatchesLocalButFetchesAFraction() {
        String q = "SELECT ?o WHERE { <http://ex/s10000> <http://ex/p> ?o }";

        // Ground truth: the same query answered from the in-memory image.
        String local;
        try (Rete rete = Rete.load()) {
            local = rete.query(image, q);
        }

        try (Rete rete = Rete.openRemote(url)) {
            String remote = rete.queryRemote(q);
            assertEquals(local, remote, "remote answer must equal the in-memory answer");
            assertTrue(remote.contains("http://ex/o10000"), remote);

            long fetched = rete.bytesFetched();
            assertTrue(fetched > 0, "a remote query must fetch something");
            assertTrue(
                    fetched < image.length,
                    "point query fetched " + fetched + " of " + image.length
                            + " bytes — expected a fraction, not the whole file");
        }
    }

    @Test
    void remoteNamedGraphs() throws IOException {
        // Build a small quad dataset and serve it from a second context.
        byte[] quads;
        try (Rete rete = Rete.load()) {
            quads =
                    rete.build(
                            "<http://ex/a> <http://ex/p> \"x\" <http://ex/g1> .\n"
                                    + "<http://ex/b> <http://ex/p> \"y\" <http://ex/g2> .\n",
                            "nq");
        }
        RangeHandler qh = new RangeHandler(quads);
        server.createContext("/quads.rete", qh);
        URI qurl = URI.create("http://127.0.0.1:" + server.getAddress().getPort() + "/quads.rete");

        try (Rete rete = Rete.openRemote(qurl)) {
            assertEquals(
                    java.util.Set.of("<http://ex/g1>", "<http://ex/g2>"),
                    new java.util.HashSet<>(rete.graphsRemote()));

            List<String[]> g1 = rete.scanInGraphRemote("<http://ex/g1>", null, null, null);
            assertEquals(1, g1.size());
            assertEquals("<http://ex/a>", g1.get(0)[0]);
        }
    }

    /**
     * A streaming cursor over HTTP is the same cursor — the transport is the
     * only difference. One row suspends the scan after a single batch; drained,
     * it returns every quad.
     */
    @Test
    void remoteCursorStreams() {
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "64");
        try {
            try (Rete rete = Rete.openRemote(url)) {
                try (QuadCursor rows = rete.scanCursor(null, null, null)) {
                    assertTrue(rows.hasNext());
                    assertEquals("<http://ex/p>", rows.next()[1]);
                }
                assertEquals(32, rete.rowsStreamed(), "one row cost more than the first batch");
                assertEquals(0, rete.openCursorCount());
            }
            try (Rete rete = Rete.openRemote(url)) {
                int n = 0;
                try (QuadCursor rows = rete.scanCursor(null, null, null)) {
                    while (rows.hasNext()) {
                        rows.next();
                        n++;
                    }
                }
                assertEquals(N, n, "a drained remote cursor must return every quad");
                assertTrue(rete.bytesFetched() > 0);
            }
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /** Minimal HTTP handler that honours {@code Range: bytes=start-end} with 206. */
    private static final class RangeHandler implements HttpHandler {
        private final byte[] data;
        final AtomicLong served = new AtomicLong();

        RangeHandler(byte[] data) {
            this.data = data;
        }

        @Override
        public void handle(HttpExchange ex) throws IOException {
            String range = ex.getRequestHeaders().getFirst("Range");
            byte[] body;
            int status;
            if (range != null && range.startsWith("bytes=")) {
                String[] parts = range.substring("bytes=".length()).split("-", 2);
                long start = Long.parseLong(parts[0]);
                long end =
                        parts.length > 1 && !parts[1].isEmpty()
                                ? Long.parseLong(parts[1])
                                : data.length - 1L;
                end = Math.min(end, data.length - 1L);
                int len = (int) (end - start + 1);
                body = Arrays.copyOfRange(data, (int) start, (int) start + len);
                ex.getResponseHeaders().set("Accept-Ranges", "bytes");
                ex.getResponseHeaders()
                        .set("Content-Range", "bytes " + start + "-" + end + "/" + data.length);
                status = 206;
            } else {
                body = data;
                status = 200;
            }
            served.addAndGet(body.length);
            ex.sendResponseHeaders(status, body.length);
            try (OutputStream os = ex.getResponseBody()) {
                os.write(body);
            }
        }
    }
}
