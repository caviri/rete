// Collect assertions instead of dying on the first one.
//
// The gate runner (tests/gate/run.mjs) decides pass/fail from the LAST JSON
// object a check prints — `lastJson(stdout).verdict === "PASS"` — not from the
// exit code. A check written as a stack of bare `node:assert` calls therefore
// throws before it prints anything, the runner finds no verdict, and the CI log
// gets a 160-character slice of a Node stack trace. That is precisely the wrong
// 160 characters: when a hard-coded tripwire goes stale (the catalog counts do,
// routinely) the log says a number is wrong without ever saying WHICH number, or
// what it is now — the one fact needed to fix it.
//
// So every assertion goes through a collector, and the check ALWAYS prints a
// verdict: PASS with its usual payload, or FAIL carrying `failures[]` with the
// actual and expected value of each failed check. The exit code stays honest
// (1 on failure) so standalone runs and non-gate callers behave as before.
//
// Usage:
//   const t = expect("test_catalog_matrix");
//   t.equal("allQueries", all.length, 676, "every query must be in the matrix");
//   t.finish({ allQueries: all.length });   // prints PASS or FAIL, sets exit code
import { isDeepStrictEqual } from "node:util";

const MAX = 160; // per-value cap: a note must stay readable in a one-line log

// Keep small values structured (a number stays a number, so `actual: 676` is
// machine-readable); summarize anything that would flood the log.
function brief(value) {
  if (value === undefined) return "(absent)";
  if (value === null || typeof value === "number" || typeof value === "boolean") return value;
  let text;
  try { text = typeof value === "string" ? value : JSON.stringify(value); } catch (e) { text = String(value); }
  if (text === undefined) text = String(value);
  if (text.length <= MAX) return value;
  return `${text.slice(0, MAX)}… (${text.length} chars)`;
}

// For two arrays, the useful report is WHERE they diverge — not both dumps.
function firstDifference(actual, expected) {
  if (Array.isArray(actual) && Array.isArray(expected)) {
    const shared = Math.min(actual.length, expected.length);
    for (let i = 0; i < shared; i++) {
      if (!isDeepStrictEqual(actual[i], expected[i])) {
        return { at: `[${i}]`, actual: actual[i], expected: expected[i] };
      }
    }
    if (actual.length !== expected.length) {
      return { at: `[${shared}]`, actual: actual[shared], expected: expected[shared] };
    }
  }
  return { at: "", actual, expected };
}

// One failure as one line — this is what the runner ends up showing, so it has
// to name the check and both values without wrapping.
function oneLine(failure) {
  const at = failure.at ? failure.at : "";
  const values = "actual" in failure
    ? `actual=${JSON.stringify(failure.actual)} expected=${JSON.stringify(failure.expected)}`
    : String(failure.error || "").split("\n")[0];
  const note = failure.note ? ` — ${failure.note}` : "";
  return `${failure.check}${at} ${values}${note}`.trim().replace(/\s+/g, " ").slice(0, 220);
}

// Everything written to stderr must be BRACE-FREE. The runner merges a child's
// streams and takes the last parseable {…} as the verdict, so a stray `{"a":1}`
// inside a human message would out-rank the real verdict object on stdout.
const plain = (text) => String(text).replace(/[{}]/g, "");

export function expect(label) {
  const failures = [];
  const push = (failure) => {
    for (const key of Object.keys(failure)) if (failure[key] === undefined) delete failure[key];
    failures.push(failure);
    return false;
  };

  return {
    failures,
    get failed() { return failures.length > 0; },

    equal(check, actual, expected, note) {
      if (Object.is(actual, expected)) return true;
      return push({ check, actual: brief(actual), expected: brief(expected), note });
    },

    deepEqual(check, actual, expected, note) {
      if (isDeepStrictEqual(actual, expected)) return true;
      const diff = firstDifference(actual, expected);
      const lengths = Array.isArray(actual) && Array.isArray(expected)
        ? { actualLength: actual.length, expectedLength: expected.length }
        : {};
      return push({
        check, at: diff.at || undefined, ...lengths,
        actual: brief(diff.actual), expected: brief(diff.expected), note,
      });
    },

    // A boolean property: there is no "expected value" beyond `true`, so the
    // note carries the explanation.
    ok(check, condition, note) {
      if (condition) return true;
      return push({ check, actual: false, expected: true, note });
    },

    match(check, actual, pattern, note) {
      if (pattern.test(String(actual))) return true;
      return push({ check, actual: brief(String(actual)), expected: `matches ${pattern}`, note });
    },

    throws(check, fn, pattern, note) {
      let error = null;
      try { fn(); } catch (e) { error = e; }
      if (!error) return push({ check, actual: "did not throw", expected: `throws ${pattern || "an error"}`, note });
      const message = String((error && error.message) || error);
      if (pattern && !pattern.test(message)) {
        return push({ check, actual: brief(message), expected: `throws ${pattern}`, note });
      }
      return true;
    },

    // An unexpected exception is still a result: record it rather than letting
    // the module die verdict-less.
    threw(check, error) {
      return push({ check, error: String((error && error.stack) || error).slice(0, 600) });
    },

    fail(check, note, extra = {}) {
      return push({ check, note, ...extra });
    },

    // PASS prints exactly `{verdict:"PASS", ...payload}` — the shape the runner
    // and its log lines already read. FAIL puts `failures` FIRST, because the
    // runner shows `JSON.stringify(j).slice(0, 160)` (the head) for child checks
    // and the TAIL of stderr for execSync ones — so stderr gets exactly one line,
    // the compact summary, and both ends of the clip carry actual/expected.
    finish(payload = {}, { indent = 0 } = {}) {
      if (!failures.length) {
        console.log(JSON.stringify({ verdict: "PASS", ...payload }, null, indent));
        return true;
      }
      console.log(JSON.stringify({ verdict: "FAIL", failures, ...payload }, null, indent));
      console.error(plain(`FAIL ${label}: ${failures.map(oneLine).join("; ")}`));
      // exitCode, not exit(): a pending stdout write to a pipe can be truncated
      // by process.exit(), which would eat the very JSON the runner parses.
      process.exitCode = 1;
      return false;
    },
  };
}
