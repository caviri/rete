package io.github.caviri.rete.rdf4j;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.github.caviri.rete.Rete;
import java.net.URI;
import java.util.ArrayList;
import java.util.List;
import org.eclipse.rdf4j.model.IRI;
import org.eclipse.rdf4j.model.Resource;
import org.eclipse.rdf4j.model.Statement;
import org.eclipse.rdf4j.model.Value;
import org.eclipse.rdf4j.model.ValueFactory;
import org.eclipse.rdf4j.model.impl.SimpleValueFactory;
import org.eclipse.rdf4j.query.BindingSet;
import org.eclipse.rdf4j.query.impl.MapBindingSet;
import org.eclipse.rdf4j.rio.helpers.NTriplesUtil;

/**
 * A <b>fast path</b> that evaluates SPARQL with rete's own engine (whole-query
 * planning and coalesced range reads) and returns results as RDF4J value types.
 *
 * <p>Contrast with {@link ReteSail}: through the Sail, RDF4J's engine drives the
 * query and calls rete once per triple pattern ({@code getStatements}) — correct
 * and fully interoperable, but over a <em>remote</em> file that is many small
 * scans. {@code ReteEngine} instead hands the whole query string to rete, which
 * plans it and fetches the minimal set of byte ranges in one shot. Same RDF4J
 * {@link BindingSet}/{@link Statement} results, fewer round-trips on remote data.
 *
 * <pre>{@code
 * try (ReteEngine engine = ReteEngine.openRemote(URI.create("https://data.example.org/x.rete"))) {
 *     for (BindingSet bs : engine.select("SELECT ?s WHERE { ?s a <http://ex/Book> }")) {
 *         System.out.println(bs.getValue("s"));
 *     }
 * }
 * }</pre>
 *
 * <p>Not thread-safe (it owns a single wasm instance). Use {@link ReteSail} when
 * you need the full RDF4J {@code Repository}/Workbench integration; use this when
 * you just want to run SPARQL and want rete's planner, especially over HTTP.
 */
public final class ReteEngine implements AutoCloseable {

    private static final ObjectMapper JSON = new ObjectMapper();

    private final ValueFactory vf = SimpleValueFactory.getInstance();
    private final Rete engine;
    private final byte[] image; // null when remote
    private final boolean remote;

    private ReteEngine(Rete engine, byte[] image, boolean remote) {
        this.engine = engine;
        this.image = image;
        this.remote = remote;
    }

    /** Open over an in-memory {@code .rete} image. */
    public static ReteEngine open(byte[] reteImage) {
        return new ReteEngine(Rete.load(), reteImage, false);
    }

    /** Open a remote {@code .rete} for lazy, range-read querying (see {@link Rete#openRemote}). */
    public static ReteEngine openRemote(URI url) {
        return new ReteEngine(Rete.openRemote(url), null, true);
    }

    /** Run a SPARQL {@code SELECT}, returning one {@link BindingSet} per solution. */
    public List<BindingSet> select(String sparql) {
        JsonNode env = envelope(sparql);
        expectKind(env, "select");
        List<String> vars = new ArrayList<>();
        env.get("vars").forEach(v -> vars.add(v.asText()));
        List<BindingSet> rows = new ArrayList<>();
        for (JsonNode row : env.get("rows")) {
            MapBindingSet bs = new MapBindingSet();
            for (String var : vars) {
                JsonNode term = row.get(var);
                if (term != null) {
                    bs.addBinding(var, NTriplesUtil.parseValue(term.asText(), vf));
                }
            }
            rows.add(bs);
        }
        return rows;
    }

    /** Run a SPARQL {@code ASK}. */
    public boolean ask(String sparql) {
        JsonNode env = envelope(sparql);
        expectKind(env, "ask");
        return env.get("boolean").asBoolean();
    }

    /** Run a SPARQL {@code CONSTRUCT}/{@code DESCRIBE}, returning the triples. */
    public List<Statement> construct(String sparql) {
        JsonNode env = envelope(sparql);
        expectKind(env, "construct");
        List<Statement> out = new ArrayList<>();
        for (JsonNode t : env.get("triples")) {
            Resource s = (Resource) NTriplesUtil.parseValue(t.get(0).asText(), vf);
            IRI p = (IRI) NTriplesUtil.parseValue(t.get(1).asText(), vf);
            Value o = NTriplesUtil.parseValue(t.get(2).asText(), vf);
            out.add(vf.createStatement(s, p, o));
        }
        return out;
    }

    /** The dataset's named graphs, as RDF4J resources. */
    public List<Resource> graphs() {
        List<String> raw = remote ? engine.graphsRemote() : engine.graphs(image);
        List<Resource> out = new ArrayList<>(raw.size());
        for (String g : raw) {
            out.add((Resource) NTriplesUtil.parseValue(g, vf));
        }
        return out;
    }

    @Override
    public void close() {
        engine.close();
    }

    /** Evaluate through rete's engine and parse the result envelope. */
    private JsonNode envelope(String sparql) {
        String json = remote ? engine.queryRemote(sparql) : engine.query(image, sparql);
        try {
            return JSON.readTree(json);
        } catch (com.fasterxml.jackson.core.JsonProcessingException e) {
            throw new IllegalStateException("could not parse rete result envelope: " + json, e);
        }
    }

    private static void expectKind(JsonNode env, String kind) {
        JsonNode k = env.get("kind");
        if (k == null || !kind.equals(k.asText())) {
            throw new IllegalArgumentException(
                    "expected a " + kind.toUpperCase() + " query but engine returned kind="
                            + (k == null ? "?" : k.asText()));
        }
    }
}
