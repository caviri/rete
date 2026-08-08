package io.github.caviri.rete.rdf4j;

import io.github.caviri.rete.Rete;
import java.net.URI;
import java.nio.file.Path;
import org.eclipse.rdf4j.model.ValueFactory;
import org.eclipse.rdf4j.model.impl.SimpleValueFactory;
import org.eclipse.rdf4j.sail.SailConnection;
import org.eclipse.rdf4j.sail.SailException;
import org.eclipse.rdf4j.sail.helpers.AbstractSail;

/**
 * A <b>read-only</b> RDF4J {@link org.eclipse.rdf4j.sail.Sail} backed by a rete
 * {@code .rete} image, so the whole RDF4J stack — {@code SailRepository},
 * SPARQL, RDF4J Workbench, Rio — can query a rete file. The rete engine runs
 * in-JVM as WebAssembly (Chicory, via {@code rete-client}); this Sail exposes it
 * as a triple source and lets RDF4J's own SPARQL engine evaluate queries over
 * it.
 *
 * <pre>{@code
 * byte[] file = Files.readAllBytes(Path.of("dataset.rete"));
 * Repository repo = new SailRepository(new ReteSail(file));
 * repo.init();
 * try (RepositoryConnection conn = repo.getConnection()) {
 *     TupleQueryResult r = conn.prepareTupleQuery("SELECT * WHERE { ?s ?p ?o }").evaluate();
 * }
 * }</pre>
 *
 * <p><b>Scope:</b> read-only. Three sources, in increasing order of what they
 * can handle:
 *
 * <ul>
 *   <li>{@code new ReteSail(byte[])} — an in-memory image. Every connection
 *       copies it into wasm linear memory per scan, so this is for small files
 *       only; above a few hundred megabytes it fails with {@code out of memory}
 *       no matter how much JVM heap there is.</li>
 *   <li>{@code new ReteSail(Path)} — a file on disk, read <b>lazily by range</b>
 *       through a {@code FileChannel}. Nothing but the blocks a query touches
 *       enters memory, so file size stops being a limit.</li>
 *   <li>{@code new ReteSail(URI)} — the same, over HTTP {@code Range} requests
 *       (206 / {@code Content-Range}).</li>
 * </ul>
 *
 * <p>Each {@link SailConnection} owns its own wasm instance — for a {@code Path}
 * or {@code URI} Sail, a resident handle whose block cache stays warm across the
 * connection's scans — so connections are independent across threads.
 */
public class ReteSail extends AbstractSail {

    private final byte[] image; // null unless constructed from bytes
    private final URI url; // null unless constructed from a URI
    private final Path path; // null unless constructed from a Path
    private final ValueFactory valueFactory = SimpleValueFactory.getInstance();

    /**
     * Wrap an in-memory {@code .rete} image. The array is copied defensively.
     * Prefer {@link #ReteSail(Path)} for anything large — see the class notes.
     */
    public ReteSail(byte[] reteImage) {
        this.image = reteImage.clone();
        this.url = null;
        this.path = null;
    }

    /**
     * Query a {@code .rete} on disk with lazy range reads: the file is not read
     * into memory, only the byte ranges each query touches are.
     */
    public ReteSail(Path path) {
        this.image = null;
        this.url = null;
        this.path = path;
    }

    /**
     * Query a remote {@code .rete} over HTTP with lazy range reads. The resource
     * must support HTTP {@code Range} requests (206 / {@code Content-Range}).
     */
    public ReteSail(URI url) {
        this.image = null;
        this.url = url;
        this.path = null;
    }

    /** The wrapped image, or {@code null} for a range-read Sail. */
    byte[] image() {
        return image;
    }

    /** Open the engine this Sail's source calls for. */
    Rete openEngine() {
        if (url != null) {
            return Rete.openRemote(url);
        }
        if (path != null) {
            return Rete.openFile(path);
        }
        return Rete.load();
    }

    /** Whether this Sail reads its source by range (rather than from an image). */
    boolean isRanged() {
        return url != null || path != null;
    }

    @Override
    protected void shutDownInternal() throws SailException {
        // Nothing global to release: each connection owns its wasm instance.
    }

    @Override
    protected SailConnection getConnectionInternal() throws SailException {
        return new ReteSailConnection(this);
    }

    @Override
    public boolean isWritable() {
        return false;
    }

    @Override
    public ValueFactory getValueFactory() {
        return valueFactory;
    }
}
