const SOURCE_URL = "/target/bench/chemotion-pinned-6cefd111.rete";
const SOURCE_BYTES = 7_566_404;
const SOURCE_SHA256 = "b7cca2e3ebe5364e767fb1f34c138d7e100b3997db172357eb4ecf3a9adfa83a";
const QUERY = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?name WHERE { ?sub rdfs:subClassOf+ <http://purl.obolibrary.org/obo/CHMO_0000228> ; rdfs:label ?name } ORDER BY ?name LIMIT 200";

function hex(bytes) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function sha256(bytes) {
  return hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
}

self.onmessage = async ({ data }) => {
  let graph;
  try {
    const sampleCount = data?.samples;
    const variant = data?.variant;
    if (!Number.isInteger(sampleCount) || sampleCount < 1) {
      throw new Error("samples must be a positive integer");
    }
    if (variant !== "baseline" && variant !== "candidate") {
      throw new Error("variant must be baseline or candidate");
    }

    self.postMessage({ kind: "progress", message: "Loading the pinned Chemotion file…" });
    const response = await fetch(SOURCE_URL, { cache: "no-store" });
    if (!response.ok) throw new Error(`source fetch failed: HTTP ${response.status}`);
    const source = await response.arrayBuffer();
    const sourceHash = await sha256(source);
    if (source.byteLength !== SOURCE_BYTES || sourceHash !== SOURCE_SHA256) {
      throw new Error(
        `source identity mismatch: ${source.byteLength} bytes, SHA-256 ${sourceHash}`,
      );
    }

    self.postMessage({ kind: "progress", message: "Opening WASM graph and warming the query…" });
    const wasm = await import(`/target/path-bench/${variant}-pkg/rete_wasm.js`);
    await wasm.default();
    graph = new wasm.Graph(new Uint8Array(source));
    const expected = graph.query(QUERY, "json");
    const samplesMs = [];

    self.postMessage({ kind: "progress", message: `Running ${sampleCount} timed queries…` });
    for (let sample = 0; sample < sampleCount; sample += 1) {
      const start = performance.now();
      const output = graph.query(QUERY, "json");
      samplesMs.push(performance.now() - start);
      if (output !== expected) throw new Error(`query output changed at sample ${sample + 1}`);
    }

    samplesMs.sort((left, right) => left - right);
    const p90Index = Math.max(0, Math.ceil(0.9 * samplesMs.length) - 1);
    const outputBytes = new TextEncoder().encode(expected);
    self.postMessage({
      kind: "result",
      report: {
        variant,
        browser: navigator.userAgent,
        source: { url: SOURCE_URL, bytes: source.byteLength, sha256: sourceHash },
        query: QUERY,
        samplesMs,
        medianMs: samplesMs[Math.floor(samplesMs.length / 2)],
        p90Ms: samplesMs[p90Index],
        outputBytes: outputBytes.byteLength,
        outputSha256: await sha256(outputBytes),
      },
    });
  } catch (error) {
    self.postMessage({
      kind: "error",
      error: error instanceof Error ? `${error.name}: ${error.message}\n${error.stack ?? ""}` : String(error),
    });
  } finally {
    graph?.free();
  }
};
