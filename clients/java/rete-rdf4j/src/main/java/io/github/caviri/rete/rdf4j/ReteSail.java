package io.github.caviri.rete.rdf4j;

import java.net.URI;
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
 * <p><b>Scope:</b> read-only. Works over an in-memory image <em>or</em> a remote
 * URL — {@code new ReteSail(uri)} queries a {@code .rete} over HTTP with lazy
 * range reads (only the bytes each query touches are fetched). Each
 * {@link SailConnection} owns its own wasm instance (for a remote Sail, a
 * resident handle whose block cache stays warm across the connection's scans),
 * so connections are independent across threads.
 */
public class ReteSail extends AbstractSail {

    private final byte[] image; // null when remote
    private final URI url; // null when local
    private final ValueFactory valueFactory = SimpleValueFactory.getInstance();

    /**
     * Wrap an in-memory {@code .rete} image. The array is copied defensively.
     */
    public ReteSail(byte[] reteImage) {
        this.image = reteImage.clone();
        this.url = null;
    }

    /**
     * Query a remote {@code .rete} over HTTP with lazy range reads. The resource
     * must support HTTP {@code Range} requests (206 / {@code Content-Range}).
     */
    public ReteSail(URI url) {
        this.image = null;
        this.url = url;
    }

    /** The wrapped image (local Sail), or {@code null} when remote. */
    byte[] image() {
        return image;
    }

    /** The remote URL (remote Sail), or {@code null} when local. */
    URI url() {
        return url;
    }

    boolean isRemote() {
        return url != null;
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
