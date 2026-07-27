package io.github.caviri.rete;

/**
 * Thrown when the rete engine reports an error — a malformed {@code .rete}
 * image, a SPARQL parse/eval failure, invalid RDF handed to
 * {@link Rete#build(String, String)}, and so on. The message is the engine's
 * own error text, forwarded verbatim from the wasm module.
 */
public class ReteException extends RuntimeException {

    private static final long serialVersionUID = 1L;

    public ReteException(String message) {
        super(message);
    }
}
