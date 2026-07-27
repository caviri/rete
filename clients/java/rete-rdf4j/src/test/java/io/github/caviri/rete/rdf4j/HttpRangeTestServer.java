package io.github.caviri.rete.rdf4j;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.URI;
import java.util.Arrays;

/**
 * A tiny in-process HTTP server that serves one {@code byte[]} with {@code Range}
 * support (HTTP 206 / {@code Content-Range}) on an ephemeral loopback port — the
 * fixture for the remote (lazy) tests. {@link AutoCloseable} for try-with-resources.
 */
final class HttpRangeTestServer implements AutoCloseable {

    private final HttpServer server;
    private final URI uri;

    HttpRangeTestServer(byte[] data) throws IOException {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/data.rete", ex -> handle(ex, data));
        server.start();
        uri = URI.create("http://127.0.0.1:" + server.getAddress().getPort() + "/data.rete");
    }

    URI uri() {
        return uri;
    }

    @Override
    public void close() {
        server.stop(0);
    }

    private static void handle(HttpExchange ex, byte[] data) throws IOException {
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
        ex.sendResponseHeaders(status, body.length);
        try (OutputStream os = ex.getResponseBody()) {
            os.write(body);
        }
    }
}
