package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.junit.jupiter.api.Test;

/**
 * End-to-end test of the pure-Java client: it drives the wasm engine to build a
 * tiny graph from N-Triples and then query it — no committed binary fixture, no
 * native library, no network. If this passes, the whole path (Chicory loads the
 * import-free wasm → linear-memory round-trips → rete-core opens and evaluates)
 * works.
 */
class ReteTest {

    /** A three-triple graph: one book, its title, and its author. */
    private static final String NT =
            "<http://example.org/book1> <http://purl.org/dc/terms/title> \"Rete\" .\n"
                    + "<http://example.org/book1> <http://purl.org/dc/terms/creator> <http://example.org/alice> .\n"
                    + "<http://example.org/alice> <http://xmlns.com/foaf/0.1/name> \"Alice\" .\n";

    @Test
    void reportsEngineVersion() {
        try (Rete rete = Rete.load()) {
            assertEquals("0.5.2", rete.version());
        }
    }

    @Test
    void aBuildIsFormat0x06WithABuildRecord() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NT, "nt");
            // "RETE", then the format byte: 0x06.
            assertEquals(6, file[4], "the Java client writes format 0x06");
            // The build record (kind 7) sits beside the card, uncompressed JSON.
            String text = new String(file, java.nio.charset.StandardCharsets.ISO_8859_1);
            assertTrue(text.contains("\"front_end\":\"java\""), "no build record naming the Java client");
            assertTrue(text.contains("\"name\":\"rete-core\""), "no engine in the build record");
            assertTrue(rete.info(file).contains("\"quads\":3"));
        }
    }

    /**
     * A tiny Jelly 1.1 file (3 quads, one in the default graph), written by hand
     * and checked against jelly-cli; the other clients use the same bytes.
     */
    private static final String JELLY_HEX =
            "bb010a0e0a0c1002481050085808700278020a1752151213687474703a2f2f6578616d706c652e6f72672f0a094a071205616c6963650a094a0712056b6e6f77730a074a051203626f620a084a0612046e616d650a064a04120267310a2c5a2a1228687474703a2f2f7777772e77332e6f72672f323030312f584d4c536368656d6123696e74656765720a0c1a0a0a0208012a004a007a000a171a152a0210045a0b0a05416c6963651202656e6a0210050a0a1a085a060a0234321801";

    private static byte[] jelly() {
        byte[] out = new byte[JELLY_HEX.length() / 2];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) Integer.parseInt(JELLY_HEX.substring(2 * i, 2 * i + 2), 16);
        }
        return out;
    }

    @Test
    void buildsFromJellyBytes() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(jelly(), "jelly");
            String info = rete.info(file);
            assertTrue(info.contains("\"quads\":3"), info);
            assertTrue(info.contains("\"namedGraphs\":1"), info);
            // Jelly is binary: text is refused, and so is a truncated file.
            assertThrows(ReteException.class, () -> rete.build("not bytes", "jelly"));
            byte[] cut = java.util.Arrays.copyOf(jelly(), jelly().length - 5);
            assertThrows(ReteException.class, () -> rete.build(cut, "jelly"));
        }
    }

    @Test
    void buildsAndInspectsAGraph() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NT, "nt");
            assertTrue(file.length > 0, "build produced no bytes");

            String info = rete.info(file);
            // Three triples, four distinct terms (3 IRIs + 2 literals share none).
            assertTrue(info.contains("\"quads\":3"), "unexpected info: " + info);
        }
    }

    @Test
    void runsASelectQuery() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NT, "nt");
            String json =
                    rete.query(
                            file,
                            "SELECT ?title WHERE { <http://example.org/book1>"
                                    + " <http://purl.org/dc/terms/title> ?title }");

            assertTrue(json.contains("\"kind\":\"select\""), json);
            assertTrue(json.contains("\"vars\":[\"title\"]"), json);
            assertTrue(json.contains("Rete"), "expected the literal in the row: " + json);
        }
    }

    @Test
    void runsAJoinAndAnAskQuery() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NT, "nt");

            // Join book -> creator -> name.
            String join =
                    rete.query(
                            file,
                            "SELECT ?name WHERE {"
                                    + " <http://example.org/book1> <http://purl.org/dc/terms/creator> ?a ."
                                    + " ?a <http://xmlns.com/foaf/0.1/name> ?name }");
            assertTrue(join.contains("Alice"), "join did not resolve the author name: " + join);

            String ask =
                    rete.query(
                            file,
                            "ASK { <http://example.org/book1> <http://purl.org/dc/terms/creator>"
                                    + " <http://example.org/alice> }");
            assertTrue(ask.contains("\"kind\":\"ask\""), ask);
            assertTrue(ask.contains("\"boolean\":true"), ask);
        }
    }

    /** A dataset with a default-graph triple and two named graphs. */
    private static final String NQ =
            "<http://ex/book1> <http://ex/title> \"Rete\" .\n"
                    + "<http://ex/book1> <http://ex/shelf> \"A1\" <http://ex/libA> .\n"
                    + "<http://ex/book2> <http://ex/shelf> \"B2\" <http://ex/libB> .\n";

    // Graph identifiers are N-Triples terms ("<iri>"), the same encoding rete
    // uses for s/p/o — consistent across the whole scan surface.

    @Test
    void listsNamedGraphs() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NQ, "nq");
            Set<String> graphs = new HashSet<>(rete.graphs(file));
            assertEquals(Set.of("<http://ex/libA>", "<http://ex/libB>"), graphs);
        }
    }

    @Test
    void scanInGraphIsScoped() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NQ, "nq");

            // Default graph: only the title triple.
            List<String[]> def = rete.scanInGraph(file, null, null, null, null);
            assertEquals(1, def.size());
            assertEquals("\"Rete\"", def.get(0)[2]);

            // libA: only book1's shelf.
            List<String[]> libA = rete.scanInGraph(file, "<http://ex/libA>", null, null, null);
            assertEquals(1, libA.size());
            assertEquals("<http://ex/book1>", libA.get(0)[0]);

            // An unknown graph is empty, not an error.
            assertTrue(rete.scanInGraph(file, "<http://ex/missing>", null, null, null).isEmpty());
        }
    }

    @Test
    void scanQuadsTagsEveryGraph() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NQ, "nq");
            List<String[]> quads = rete.scanQuads(file, null, null, null);
            assertEquals(3, quads.size());

            int defaults = 0;
            Set<String> named = new HashSet<>();
            for (String[] q : quads) {
                if (q[3] == null) {
                    defaults++;
                } else {
                    named.add(q[3]);
                }
            }
            assertEquals(1, defaults, "one default-graph quad");
            assertEquals(Set.of("<http://ex/libA>", "<http://ex/libB>"), named);
        }
    }

    @Test
    void surfacesEngineErrors() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(NT, "nt");
            // A syntactically invalid query must raise, carrying the engine message.
            ReteException e =
                    assertThrows(ReteException.class, () -> rete.query(file, "SELECT ?s WHERE {"));
            assertFalse(e.getMessage().isBlank(), "error message should not be empty");
        }
    }
}
