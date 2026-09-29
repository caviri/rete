package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;

/**
 * RDF 1.2 Turtle/TriG through the Chicory engine, and the property its blank
 * nodes depend on.
 *
 * <p>The RDF 1.2 reader ({@code oxttl} 0.2) labels every anonymous blank node
 * ({@code []}, a reifier, an annotation) with a random id drawn through
 * getrandom, which this engine feeds from a non-cryptographic xorshift in
 * {@code ffi/src/lib.rs}. Uniqueness is all that labelling needs, so the test
 * that matters is the one that would catch the xorshift repeating itself: two
 * documents built in two separate engine instances must not get the same
 * blank-node labels, or merging or federating the two files would conflate
 * them.
 */
class Rdf12Test {

    private static final String RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

    private static final String TTL =
            "@prefix ex: <http://example.test/> .\n"
                    + "ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .\n"
                    + "<< ex:bob ex:knows ex:dave >> ex:source ex:wiki .\n"
                    + "ex:erin ex:knows ex:frank {| ex:since \"2020\" |} .\n"
                    + "ex:gina ex:name \"Gina\"@en--ltr .\n";

    private static final String TRIG =
            "@prefix ex: <http://example.test/> .\n"
                    + "ex:g {\n"
                    + "  ex:alice ex:says <<( ex:bob ex:knows ex:carol )>> .\n"
                    + "  << ex:bob ex:knows ex:dave >> ex:source ex:wiki .\n"
                    + "}\n";

    private static final String ANON = "[] <http://example.test/p> \"x\" .\n";

    private static final String SUBJECTS =
            "SELECT ?s WHERE { ?s <http://example.test/p> \"x\" }";

    private static final Pattern BLANK = Pattern.compile("\"(_:[A-Za-z0-9]+)\"");

    private static List<String> blankNodes(String json) {
        List<String> out = new ArrayList<>();
        Matcher m = BLANK.matcher(json);
        while (m.find()) {
            out.add(m.group(1));
        }
        return out;
    }

    @Test
    void readsRdf12Turtle() {
        try (Rete rete = Rete.load()) {
            byte[] file = rete.build(TTL, "ttl", "rdf12");
            // A triple term; a reifier (rdf:reifies + ex:source); an annotation
            // (the asserted triple + rdf:reifies + ex:since); a dir-lang literal.
            String info = rete.info(file);
            assertTrue(info.contains("\"quads\":7"), info);
            List<String> reifiers =
                    blankNodes(rete.query(file, "SELECT ?r WHERE { ?r <" + RDF + "reifies> ?t }"));
            assertEquals(2, reifiers.size(), "one reifier and one annotation");
            assertEquals(2, new HashSet<>(reifiers).size(), "reifiers must be distinct");
            String asserted =
                    rete.query(
                            file,
                            "ASK { <http://example.test/erin> <http://example.test/knows>"
                                    + " <http://example.test/frank> }");
            assertTrue(asserted.contains("\"boolean\":true"), "annotated triple is asserted: " + asserted);
        }
    }

    @Test
    void readsRdf12TriG() {
        try (Rete rete = Rete.load()) {
            String info = rete.info(rete.build(TRIG, "trig", "rdf12"));
            assertTrue(info.contains("\"quads\":3"), info);
            assertTrue(info.contains("\"namedGraphs\":1"), info);
        }
    }

    @Test
    void rdfStarDefaultIsUnchanged() {
        try (Rete rete = Rete.load()) {
            ReteException refused = assertThrows(ReteException.class, () -> rete.build(TTL, "ttl"));
            assertTrue(refused.getMessage().contains("rdf12"), refused.getMessage());
            assertThrows(ReteException.class, () -> rete.build(TTL, "ttl", "rdf13"));
            String nt = "<http://example.test/a> <http://example.test/p> \"x\" .\n";
            assertArrayEquals(rete.build(nt, "nt"), rete.build(nt, "nt", "rdf-star"));
            assertArrayEquals(rete.build(nt, "nt"), rete.build(nt, "nt", ""));
        }
    }

    @Test
    void blankNodesOfSeparateParsesStayDistinctAcrossInstances() {
        List<String> labels = new ArrayList<>();
        try (Rete a = Rete.load();
                Rete b = Rete.load()) {
            // Two parses in one instance, and two in a second, fresh instance
            // (fresh linear memory, as each RDF4J connection gets): through the
            // RDF 1.2 reader (getrandom 0.3) and the RDF-star one (getrandom 0.2).
            labels.addAll(blankNodes(a.query(a.build(ANON, "ttl", "rdf12"), SUBJECTS)));
            labels.addAll(blankNodes(a.query(a.build(ANON, "ttl", "rdf12"), SUBJECTS)));
            labels.addAll(blankNodes(b.query(b.build(ANON, "ttl", "rdf12"), SUBJECTS)));
            labels.addAll(blankNodes(b.query(b.build(ANON, "ttl"), SUBJECTS)));
            assertEquals(4, labels.size(), "one blank node per document: " + labels);
            assertEquals(4, new HashSet<>(labels).size(), "blank nodes collided: " + labels);

            // Merge the four documents into one store: still four subjects.
            StringBuilder merged = new StringBuilder();
            for (String l : labels) {
                merged.append(l).append(" <http://example.test/p> \"x\" .\n");
            }
            List<String> subjects = blankNodes(a.query(a.build(merged.toString(), "nt"), SUBJECTS));
            assertEquals(4, new HashSet<>(subjects).size(), "merge conflated blank nodes: " + subjects);
        }
    }
}
