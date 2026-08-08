package io.github.caviri.rete;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * The engine runs compiled by default and interpreted when
 * {@code -Drete.chicory.interpreter=true} is set — and answers identically
 * either way.
 *
 * <p>The mode is resolved once per JVM (compiling the module is the expensive
 * part, so it is cached), which is why the interpreted case cannot be a
 * {@code @Test} toggling a property mid-run: it needs its own JVM. The Maven
 * build therefore runs this class twice — once in the default reactor test run
 * (compiled) and once more in the {@code interpreter} surefire execution — so
 * both branches are exercised on every build.
 */
class ExecutionModeTest {

    private static final String NT =
            "<http://example.org/book1> <http://purl.org/dc/terms/title> \"Rete\" .\n";

    private static final String Q =
            "SELECT ?title WHERE { ?s <http://purl.org/dc/terms/title> ?title }";

    @Test
    void modeFollowsTheSystemProperty() {
        boolean wantInterpreter = Boolean.getBoolean(Rete.INTERPRETER_PROPERTY);
        try (Rete rete = Rete.load()) {
            assertEquals(
                    !wantInterpreter,
                    Rete.compiled(),
                    "execution mode should follow -D" + Rete.INTERPRETER_PROPERTY);
            // ...and the answers are the same in either mode.
            byte[] file = rete.build(NT, "nt");
            assertTrue(rete.query(file, Q).contains("Rete"));
        }
    }

    @Test
    void instancesAreIndependentThoughTheyShareOneParsedModule() {
        try (Rete a = Rete.load();
                Rete b = Rete.load()) {
            assertNotSame(a, b);
            // Built in one instance, queried in the other: the module is shared,
            // the linear memories are not.
            byte[] file = a.build(NT, "nt");
            assertTrue(b.query(file, Q).contains("Rete"));
            // ...and the first instance still works after the second one ran.
            assertTrue(a.query(file, Q).contains("Rete"));
            assertEquals(a.version(), b.version());
        }
    }
}
