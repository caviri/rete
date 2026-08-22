package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Exercises the <b>local file, lazy</b> path: {@link Rete#openFile(Path)}.
 *
 * <p>The same three things {@link ReteRemoteTest} proves over HTTP — same
 * answers as in-memory, only a fraction of the file read, named graphs work —
 * because it is the same reader with a {@code FileChannel} underneath instead of
 * a socket. The byte-fraction assertion is the one that matters: it is what
 * makes a file bigger than wasm32's address space openable at all.
 */
class ReteFileTest {

    private static final int N = 20_000;

    private static byte[] image;

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

    private static Path write(Path dir, String name, byte[] bytes) throws IOException {
        Path p = dir.resolve(name);
        Files.write(p, bytes);
        return p;
    }

    @Test
    void fileInfoMatchesTheImage(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        String expected;
        try (Rete rete = Rete.load()) {
            expected = rete.info(image);
        }
        try (Rete rete = Rete.openFile(file)) {
            assertEquals(expected, rete.info());
        }
    }

    @Test
    void fileQueryMatchesTheImageButReadsAFraction(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        String q = "SELECT ?o WHERE { <http://ex/s10000> <http://ex/p> ?o }";

        String inMemory;
        try (Rete rete = Rete.load()) {
            inMemory = rete.query(image, q);
        }

        try (Rete rete = Rete.openFile(file)) {
            String lazy = rete.query(q);
            assertEquals(inMemory, lazy, "a lazily read file must answer exactly as the image does");
            assertTrue(lazy.contains("http://ex/o10000"), lazy);

            long read = rete.bytesRead();
            assertTrue(read > 0, "a query must read something");
            assertTrue(
                    read < image.length,
                    "point query read " + read + " of " + image.length
                            + " bytes — expected a fraction, not the whole file");
        }
    }

    @Test
    void fileNamedGraphs(@TempDir Path dir) throws IOException {
        byte[] quads;
        try (Rete rete = Rete.load()) {
            quads =
                    rete.build(
                            "<http://ex/a> <http://ex/p> \"x\" <http://ex/g1> .\n"
                                    + "<http://ex/b> <http://ex/p> \"y\" <http://ex/g2> .\n",
                            "nq");
        }
        Path file = write(dir, "quads.rete", quads);

        try (Rete rete = Rete.openFile(file)) {
            assertEquals(
                    java.util.Set.of("<http://ex/g1>", "<http://ex/g2>"),
                    new java.util.HashSet<>(rete.graphs()));

            List<String[]> g1 = rete.scanInGraph("<http://ex/g1>", null, null, null);
            assertEquals(1, g1.size());
            assertEquals("<http://ex/a>", g1.get(0)[0]);

            List<String[]> all = rete.scanQuads(null, null, null);
            assertEquals(2, all.size());
        }
    }

    /** {@code close()} must release the file descriptor, not just the handle. */
    @Test
    void closeReleasesTheFile(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        Rete rete = Rete.openFile(file);
        rete.info();
        rete.close();
        assertThrows(IllegalStateException.class, rete::info);
        // The descriptor is gone, so the file is free to be replaced/deleted.
        Files.delete(file);
    }

    /** An engine with no file open must say so, not read garbage. */
    @Test
    void engineWithoutAnOpenFileRejectsHandleCalls() {
        try (Rete rete = Rete.load()) {
            IllegalStateException e = assertThrows(IllegalStateException.class, rete::info);
            assertTrue(e.getMessage().contains("openFile"), e.getMessage());
        }
    }

    /** A missing file is a clean ReteException, not an NPE from deep inside. */
    @Test
    void missingFileFailsCleanly(@TempDir Path dir) {
        assertThrows(ReteException.class, () -> Rete.openFile(dir.resolve("nope.rete")));
    }

    /** The …Remote aliases must keep working on a file-backed engine. */
    @Test
    void remoteSpellingsAreAliases(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            assertEquals(rete.info(), rete.infoRemote());
            assertEquals(rete.graphs(), rete.graphsRemote());
            assertEquals(rete.bytesRead(), rete.bytesFetched());
        }
    }
}
