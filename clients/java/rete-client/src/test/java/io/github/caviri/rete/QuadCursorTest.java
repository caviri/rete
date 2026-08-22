package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.Set;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * The <b>streaming</b> scan: {@link Rete#scanCursor} and
 * {@link Rete#scanCursorInGraph}.
 *
 * <p>Three properties have to hold or the cursor is a materializing scan wearing
 * a cursor's clothes:
 *
 * <ol>
 *   <li>drained, it returns exactly what the list-returning scan returns —
 *       streaming may not change what a query answers;
 *   <li>stopping after one row costs a fraction of the bytes a full drain reads;
 *   <li>every way a cursor can end — close, exhaustion, abandonment,
 *       exception, engine close — releases its engine-side state.
 * </ol>
 */
class QuadCursorTest {

    private static final int N = 20_000;

    private static byte[] image;
    private static byte[] quads;

    @BeforeAll
    static void buildImages() {
        StringBuilder nt = new StringBuilder(N * 60);
        for (int i = 0; i < N; i++) {
            nt.append("<http://ex/s").append(i).append("> <http://ex/p").append(i % 3)
                    .append("> <http://ex/o").append(i).append("> .\n");
        }
        try (Rete rete = Rete.load()) {
            image = rete.build(nt.toString(), "nt");
            quads =
                    rete.build(
                            "<http://ex/a> <http://ex/p> \"x\" <http://ex/g1> .\n"
                                    + "<http://ex/b> <http://ex/p> \"y\" <http://ex/g2> .\n"
                                    + "<http://ex/c> <http://ex/p> \"z\" <http://ex/g2> .\n",
                            "nq");
        }
    }

    private static Path write(Path dir, String name, byte[] bytes) throws IOException {
        Path p = dir.resolve(name);
        Files.write(p, bytes);
        return p;
    }

    private static List<String> rowsOf(QuadCursor cursor) {
        List<String> out = new ArrayList<>();
        while (cursor.hasNext()) {
            out.add(Arrays.toString(cursor.next()));
        }
        return out;
    }

    /**
     * The load-bearing equivalence: whatever the batch size, a drained cursor
     * yields exactly the rows {@code scanQuads} yields.
     */
    @Test
    void drainedCursorMatchesTheListScan(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            List<String> eager = new ArrayList<>();
            for (String[] row : rete.scanQuads(null, null, null)) {
                eager.add(Arrays.toString(row));
            }
            assertEquals(N, eager.size());
            for (String batch : List.of("1", "7", "512", "1000000")) {
                System.setProperty(Rete.SCAN_BATCH_PROPERTY, batch);
                try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                    assertEquals(eager, rowsOf(cursor), "batch " + batch + " changed the scan");
                }
            }
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /** Bound patterns stream too, and agree with the eager scan as a set. */
    @Test
    void boundPatternsMatchTheListScan(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "3");
        try (Rete rete = Rete.openFile(file)) {
            String[][] patterns = {
                {"<http://ex/s99>", null, null},
                {null, "<http://ex/p1>", null},
                {null, null, "<http://ex/o1234>"},
                {"<http://ex/s99>", "<http://ex/p0>", null},
                {"<http://ex/nope>", null, null},
            };
            for (String[] pat : patterns) {
                Set<String> eager = new HashSet<>();
                for (String[] row : rete.scanQuads(pat[0], pat[1], pat[2])) {
                    eager.add(Arrays.toString(row));
                }
                try (QuadCursor cursor = rete.scanCursor(pat[0], pat[1], pat[2])) {
                    assertEquals(
                            eager,
                            new HashSet<>(rowsOf(cursor)),
                            "streamed " + Arrays.toString(pat) + " differs");
                }
            }
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /**
     * The point of the whole exercise: the engine must <b>suspend</b>, not
     * materialize. One row out of {@value #N} may cost at most one batch of
     * engine work, and the scan must come back unfinished — that is what makes
     * {@code SELECT ?s ?p ?o LIMIT 1} answerable on a file larger than memory.
     *
     * <p>Bytes are the wrong witness at unit-test scale: the block cache reads in
     * 128&nbsp;KiB blocks and a megabyte fixture is a handful of them, so the
     * fixed open cost swamps the ratio. The byte-level laziness is asserted in
     * {@code rete-core}, over a counting reader with no block cache.
     */
    @Test
    void takingOneRowSuspendsInsteadOfMaterializing(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "16");
        try (Rete rete = Rete.openFile(file)) {
            try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                assertTrue(cursor.hasNext());
                assertNotNullRow(cursor.next());
            }
            assertEquals(1, rete.batchCalls(), "one row should need exactly one batch");
            assertEquals(
                    16,
                    rete.rowsStreamed(),
                    "the engine produced " + rete.rowsStreamed() + " of " + N + " rows to answer"
                            + " one — it is still materializing");
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /**
     * The batch RAMPS: the first call is small (so {@code LIMIT 1} is cheap) and
     * doubles to the ceiling (so a drain is fast). Without it the two cases pull
     * in opposite directions.
     */
    @Test
    void theBatchRampsToTheCeiling(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "256");
        try (Rete rete = Rete.openFile(file)) {
            try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                cursor.next();
                assertEquals(32, rete.rowsStreamed(), "the first batch is not the small one");
                long[] expected = {32, 96, 224, 480, 736, 992}; // 32,64,128,256,256,256
                for (int i = 1; i < expected.length; i++) {
                    while (rete.rowsStreamed() == expected[i - 1]) {
                        cursor.next();
                    }
                    assertEquals(expected[i], rete.rowsStreamed(), "ramp step " + i);
                }
            }
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /** A batch is a floor with a bounded overshoot, and it reports "not done". */
    @Test
    void aBatchIsBoundedAndResumable(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                Rete.Batch batch = rete.nextBatch(cursor.id(), 16);
                // One triple per subject in this fixture, so no group can push
                // the batch past its floor.
                assertEquals(16, batch.rows().size());
                assertFalse(batch.done(), "the scan must come back suspended, not finished");
            }
        }
    }

    private static void assertNotNullRow(String[] row) {
        assertEquals(4, row.length);
        for (int i = 0; i < 3; i++) {
            assertTrue(row[i] != null && !row[i].isEmpty());
        }
        assertNull(row[3], "the default graph is a null context");
    }

    /** Named graphs: the quad scan tags every row, the graph scan is scoped. */
    @Test
    void namedGraphsStream(@TempDir Path dir) throws IOException {
        Path file = write(dir, "quads.rete", quads);
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "1");
        try (Rete rete = Rete.openFile(file)) {
            Set<String> all = new HashSet<>();
            try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                while (cursor.hasNext()) {
                    String[] r = cursor.next();
                    all.add(r[0] + " " + r[3]);
                }
            }
            assertEquals(
                    Set.of(
                            "<http://ex/a> <http://ex/g1>",
                            "<http://ex/b> <http://ex/g2>",
                            "<http://ex/c> <http://ex/g2>"),
                    all);

            try (QuadCursor cursor = rete.scanCursorInGraph("<http://ex/g2>", null, null, null)) {
                List<String> rows = rowsOf(cursor);
                assertEquals(2, rows.size());
            }
            // An unknown graph is empty, not an error.
            try (QuadCursor cursor = rete.scanCursorInGraph("<http://ex/nope>", null, null, null)) {
                assertFalse(cursor.hasNext());
            }
            // The default graph of a quads file is empty here.
            try (QuadCursor cursor = rete.scanCursorInGraph(null, null, null, null)) {
                assertFalse(cursor.hasNext());
            }
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    // --- lifecycle: every way a cursor ends must release it -------------------

    /** try-with-resources releases it. */
    @Test
    void closeReleasesTheCursor(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            assertEquals(0, rete.openCursorCount());
            try (QuadCursor cursor = rete.scanCursor(null, null, null)) {
                cursor.next();
                assertEquals(1, rete.openCursorCount());
            }
            assertEquals(0, rete.openCursorCount());
        }
    }

    /** Draining to exhaustion releases it without an explicit close. */
    @Test
    void exhaustionReleasesTheCursor(@TempDir Path dir) throws IOException {
        Path file = write(dir, "quads.rete", quads);
        try (Rete rete = Rete.openFile(file)) {
            QuadCursor cursor = rete.scanCursor(null, null, null);
            while (cursor.hasNext()) {
                cursor.next();
            }
            assertEquals(0, rete.openCursorCount(), "a drained cursor must release itself");
            assertThrows(NoSuchElementException.class, cursor::next);
            // close() after exhaustion is a no-op, not a double free.
            cursor.close();
            assertEquals(0, rete.openCursorCount());
        }
    }

    /**
     * ABANDONMENT — the one that leaks silently. A cursor dropped mid-scan and
     * collected must not leave engine-side state behind; the cleaner queues it
     * and the engine reaps it on its own thread.
     */
    @Test
    void abandonedCursorsAreReaped(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        System.setProperty(Rete.SCAN_BATCH_PROPERTY, "1");
        try (Rete rete = Rete.openFile(file)) {
            for (int i = 0; i < 40; i++) {
                QuadCursor cursor = rete.scanCursor(null, null, null);
                cursor.next(); // opened, partly read, then dropped
                cursor = null;
            }
            assertTrue(rete.openCursorCount() > 0, "the test did not actually open cursors");
            int open = -1;
            for (int attempt = 0; attempt < 40 && open != 0; attempt++) {
                System.gc();
                try {
                    Thread.sleep(50);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                rete.reapAbandonedCursors();
                open = rete.openCursorCount();
            }
            assertEquals(0, open, "abandoned cursors were never reaped — this is a leak");
        } finally {
            System.clearProperty(Rete.SCAN_BATCH_PROPERTY);
        }
    }

    /** Opening a cursor also reaps, so a Sail that never calls anything else is safe. */
    @Test
    void openingACursorReapsAbandonedOnes(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            for (int i = 0; i < 20; i++) {
                QuadCursor cursor = rete.scanCursor(null, null, null);
                cursor.next();
                cursor = null;
            }
            int open = Integer.MAX_VALUE;
            for (int attempt = 0; attempt < 40 && open > 1; attempt++) {
                System.gc();
                try {
                    Thread.sleep(50);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                // Opening (and closing) one cursor is the only call made.
                try (QuadCursor probe = rete.scanCursor(null, null, null)) {
                    probe.hasNext();
                    open = rete.openCursorCount();
                }
            }
            assertTrue(open <= 1, "opening a cursor did not reap the abandoned ones (" + open + ")");
        }
    }

    /** Closing the engine drops every cursor still open on it. */
    @Test
    void closingTheEngineDropsEveryCursor(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        Rete rete = Rete.openFile(file);
        QuadCursor a = rete.scanCursor(null, null, null);
        QuadCursor b = rete.scanCursor(null, null, null);
        a.next();
        b.next();
        assertEquals(2, rete.openCursorCount());
        rete.close();
        // Closing a cursor after its engine is closed must be a quiet no-op.
        a.close();
        b.close();
        Files.delete(file); // the descriptor really was released
    }

    /** An exception from the engine closes the cursor before it propagates. */
    @Test
    void engineFailureReleasesTheCursor(@TempDir Path dir) throws IOException {
        Path file = write(dir, "data.rete", image);
        try (Rete rete = Rete.openFile(file)) {
            QuadCursor cursor = rete.scanCursor(null, null, null);
            cursor.next();
            assertEquals(1, rete.openCursorCount());
            // Pull the rug out: the engine-side cursor is gone, so the next
            // batch fails. The Java cursor must release rather than wedge.
            rete.closeCursor(cursor.id());
            assertThrows(ReteException.class, () -> {
                while (cursor.hasNext()) {
                    cursor.next();
                }
            });
            assertFalse(cursor.hasNext(), "a failed cursor must stay closed");
            assertEquals(0, rete.openCursorCount());
        }
    }

    /** An engine with no file open has no cursors to give. */
    @Test
    void cursorNeedsAnOpenFile() {
        try (Rete rete = Rete.load()) {
            assertThrows(IllegalStateException.class, () -> rete.scanCursor(null, null, null));
        }
    }

}
