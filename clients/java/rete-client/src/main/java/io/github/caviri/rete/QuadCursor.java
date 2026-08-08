package io.github.caviri.rete;

import java.lang.ref.Cleaner;
import java.util.Iterator;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * A <b>streaming</b> scan over an open {@code .rete}: rows arrive a bounded
 * batch at a time and the engine never builds the whole result.
 *
 * <p>This is the difference between an unconstrained {@code (?s ?p ?o)} being
 * answerable and not. The list-returning {@link Rete#scanQuads(String, String,
 * String)} makes the engine materialize every match inside wasm32 linear memory
 * before the first row crosses the boundary — on a 26-million-quad file that
 * exhausts the 4&nbsp;GiB address space, and RDF4J issues exactly that pattern
 * for {@code SELECT ?s ?p ?o … LIMIT 1} because the {@code LIMIT} sits above the
 * triple source. A cursor pulls {@link Rete#SCAN_BATCH_PROPERTY} rows per wasm
 * call instead, so time-to-first-row and peak memory are bounded by the batch,
 * not by the graph.
 *
 * <p>Each row is a {@code String[]}{@code {subject, predicate, object, graph}}
 * of N-Triples term strings, with {@code graph == null} for the default graph.
 *
 * <p><b>Close it.</b> A cursor holds state inside the engine until released.
 * Use try-with-resources:
 *
 * <pre>{@code
 * try (QuadCursor rows = rete.scanCursor(null, null, null)) {
 *     while (rows.hasNext()) {
 *         String[] quad = rows.next();
 *     }
 * }
 * }</pre>
 *
 * Three things release it besides an explicit {@link #close()}: draining it to
 * exhaustion (the last batch closes itself), an exception from the engine
 * (closed before the exception propagates), and {@link Rete#close()}, which
 * drops every cursor on the file. If a cursor is abandoned mid-scan and
 * garbage-collected without any of those, a {@link Cleaner} queues its id on the
 * owning {@link Rete}, which releases it on the next engine call from the owning
 * thread — the cleaner thread never calls into wasm itself, because a
 * {@code Rete} owns a single wasm instance and is not thread-safe.
 *
 * <p>Not thread-safe, and no more so than the {@link Rete} it came from.
 */
public final class QuadCursor implements Iterator<String[]>, AutoCloseable {

    /** One shared cleaner for every cursor in the JVM; its thread starts lazily. */
    private static final Cleaner CLEANER = Cleaner.create();

    /**
     * Rows in the FIRST batch, before the ramp. The whole cost of the first row
     * is this many rows' worth of engine work — index tiles plus one coalesced
     * dictionary fault — so it is what {@code LIMIT 1} pays. On
     * {@code cordis.rete} (763.9&nbsp;MiB) through a {@code SailRepository}:
     * 2.16&nbsp;s at 32 rows, 3.17&nbsp;s at 256, 6.37&nbsp;s at 2048,
     * 10.99&nbsp;s at 8192.
     */
    private static final int FIRST_BATCH = 32;

    private final Rete engine;
    private final int id;
    private final int maxBatch;
    private final Reaper reaper;
    private final Cleaner.Cleanable cleanable;

    private int batch;
    private List<String[]> rows = List.of();
    private int pos;
    private boolean done;
    private boolean closed;

    QuadCursor(Rete engine, int id, int maxBatch, ConcurrentLinkedQueue<Integer> abandoned) {
        this.engine = engine;
        this.id = id;
        this.maxBatch = maxBatch;
        // Ramp the batch geometrically rather than opening at full size — the
        // same shape the engine's own tile prefetch uses, and for the same
        // reason. A consumer that stops after one row (RDF4J's `LIMIT`) pays for
        // 32 rows; one that drains reaches the full batch after six calls, which
        // is 4,064 rows of a scan that may be millions. Without the ramp the two
        // cases pull in opposite directions: full-drain throughput wants a big
        // batch, time-to-first-row wants a small one.
        this.batch = Math.min(FIRST_BATCH, maxBatch);
        this.reaper = new Reaper(abandoned, id);
        // The action must not reference `this`, or the cursor is never collected
        // and the cleaner never runs. Reaper holds the queue and an int.
        this.cleanable = CLEANER.register(this, reaper);
    }

    @Override
    public boolean hasNext() {
        if (closed) {
            return false;
        }
        while (pos >= rows.size()) {
            if (done) {
                // Drained: release now so a consumer that reads to the end owes
                // nothing. close() is idempotent, so a later one is harmless.
                close();
                return false;
            }
            fill();
        }
        return true;
    }

    @Override
    public String[] next() {
        if (!hasNext()) {
            throw new NoSuchElementException();
        }
        return rows.get(pos++);
    }

    /**
     * Release the cursor's engine-side state. Idempotent; safe after
     * {@link Rete#close()}.
     */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        rows = List.of();
        pos = 0;
        // Release on THIS thread while we are here; then defuse the cleaner so
        // the GC path does not queue an id that is already gone.
        if (reaper.released.compareAndSet(false, true)) {
            engine.closeCursor(id);
        }
        cleanable.clean();
    }

    /** The engine-side cursor id — for tests that assert the leak accounting. */
    int id() {
        return id;
    }

    /** Pull one batch, closing the cursor if the engine throws. */
    private void fill() {
        try {
            // A batch may legitimately be empty while not done: `next` walks a
            // bounded number of graphs per call, so a pattern that misses many
            // named graphs reports "nothing yet". Looping here is the whole
            // handling — each call advances the scan.
            Rete.Batch b = engine.nextBatch(id, batch);
            rows = b.rows();
            pos = 0;
            done = b.done();
            batch = Math.min(batch * 2, maxBatch);
        } catch (RuntimeException | Error e) {
            close();
            throw e;
        }
    }

    /**
     * The GC-without-close path. Holds no reference to the cursor (that would
     * pin it forever) — only the owning engine's abandoned-id queue and the id.
     * Running it merely queues; the engine reaps on its own thread.
     */
    private static final class Reaper implements Runnable {
        private final ConcurrentLinkedQueue<Integer> queue;
        private final int id;
        private final AtomicBoolean released = new AtomicBoolean();

        Reaper(ConcurrentLinkedQueue<Integer> queue, int id) {
            this.queue = queue;
            this.id = id;
        }

        @Override
        public void run() {
            if (released.compareAndSet(false, true)) {
                queue.add(id);
            }
        }
    }
}
