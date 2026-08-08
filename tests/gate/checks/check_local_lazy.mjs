// A LOCAL .rete must open the way a remote one does — a handful of byte ranges
// — instead of `file.arrayBuffer()`, which materializes the whole file in a JS
// buffer, copies it into wasm, and decodes every dictionary chunk before a
// single row is answered (issue #102).
//
// Two independent measurements, because they fail differently:
//
//   1. THE PAGE never materializes the file. `Blob.prototype.arrayBuffer`,
//      `Blob.prototype.text` and `FileReader.readAsArrayBuffer` are patched
//      before the app boots and every call is recorded with the blob's size. A
//      regression to `file.arrayBuffer()` shows up here as one whole-file read,
//      whatever the engine then reports.
//   2. THE ENGINE reads a fraction. The playground's own counters (the same
//      `stats()` deltas #qmeta prints) say how many bytes and range requests the
//      open + query actually cost. Read exactly, via `window.__reteOpenFacts()`.
//
// Both engines are exercised: the page ships an asyncify build AND a sync one,
// and only the sync path can use `FileReaderSync`… except that a local read
// never suspends, so both must work. Forcing each in turn is the only way to
// know that rather than assume it.
//
// The fixture is built IN THE PAGE (wasm_bindgen.build) and handed to the file
// input as an in-memory File. That is deliberate: the tracked .rete fixtures are
// 2–19 KB, and the block cache reads a small file in ONE block — so a byte ratio
// measured on one of them would be vacuously ~100% and would prove nothing. This
// builds a multi-MB file, where "reads a fraction" is a real claim.
import { launchBrowser } from "./_browser.mjs";
import { expect } from "./_expect.mjs";

const PGPORT = process.env.PGPORT || "8090";
const t = expect("check_local_lazy");

// 90k triples over 45k subjects, each carrying a LONG pseudo-random literal
// (seeded LCG → base36), so front-coding and zstd cannot fold the dictionary
// away. Size comes from the literals, not the triple count, deliberately: the
// file has to be several MB for the byte ratio below to mean anything, while the
// in-page build has to finish on a 2-core CI runner. A compressible filler gave
// a ~1 MB file where a bounded read is most of it; 110k short rows gave a good
// ratio and timed out in CI. Seeded, so the bytes are identical every run.
const SUBJECTS = 45000;
const NOISE_CHUNKS = 90;   // ≈ 990 characters per literal

// One bounded question: a single subject, both of its triples. It must touch a
// term lookup and one index tile, not the graph.
const QUERY = `SELECT ?p ?o WHERE { <https://example.org/gate/local/s012345> ?p ?o }`;

// Patch every way a page can turn a Blob into bytes, BEFORE the app boots.
const INSTRUMENT = () => {
  window.__blobReads = [];
  const note = (how, size) => { try { window.__blobReads.push({ how, size }); } catch (e) { /* ignore */ } };
  const ab = Blob.prototype.arrayBuffer;
  Blob.prototype.arrayBuffer = function () { note("arrayBuffer", this.size); return ab.apply(this, arguments); };
  const txt = Blob.prototype.text;
  if (txt) { Blob.prototype.text = function () { note("text", this.size); return txt.apply(this, arguments); }; }
  if (window.FileReader) {
    const rab = FileReader.prototype.readAsArrayBuffer;
    FileReader.prototype.readAsArrayBuffer = function (blob) { note("FileReader", blob && blob.size); return rab.apply(this, arguments); };
  }
};

const openPage = async (browser, { lazyAboveMB, asyncReads }) => {
  const page = await browser.newPage();
  await page.addInitScript(INSTRUMENT);
  await page.addInitScript(([mb, async]) => {
    try {
      if (mb === null) localStorage.removeItem("localLazyAboveMB");
      else localStorage.setItem("localLazyAboveMB", String(mb));
      localStorage.setItem("asyncReadsOn", async ? "1" : "0");
    } catch (e) { /* private mode */ }
  }, [lazyAboveMB, asyncReads]);
  await page.goto(`http://localhost:${PGPORT}/playground.html#dataset=causal&load=bundled&mode=sparql`,
    { waitUntil: "domcontentloaded" });
  // NOTE the `null`: waitForFunction is (fn, arg, options) — passing the
  // options object second makes it the ARGUMENT and silently keeps the 30 s
  // default, which is how this check first failed.
  await page.waitForFunction(
    () => window.PlaygroundEditor && document.getElementById("run") && !document.getElementById("run").disabled,
    null, { timeout: 90000 },
  );
  return page;
};

// Attach `bytes` as a local File and run the bounded query. Returns everything
// both measurements need.
const openLocalAndQuery = async (page, bytes) => {
  await page.setInputFiles("#loadFileInput", {
    name: "gate-local.rete", mimeType: "application/octet-stream", buffer: bytes,
  });
  // Wait for THIS file to be open, on whichever route it took. The size must be
  // matched exactly: the page boots with a bundled dataset resident, so a bare
  // `inMemoryBytes > 0` is already true before the file is even read, and the
  // whole-file assertions below would race a graph that has not been swapped yet.
  await page.waitForFunction((want) => {
    const f = window.__reteOpenFacts && window.__reteOpenFacts();
    return !!f && (f.local === true || f.inMemoryBytes === want);
  }, bytes.length, { timeout: 240000 });

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), QUERY);
  await page.evaluate(() => {
    window.__qmetaBefore = (document.getElementById("qmeta") || {}).textContent || "";
    document.getElementById("run").click();
  });
  // Wait for the RUN TO FINISH, not for rows. Waiting on rows turns "answered 0"
  // into a timeout, and a timeout says nothing about what happened — the whole
  // point of the _expect collector is that a failure names its value.
  await page.waitForFunction(() => {
    const qm = (document.getElementById("qmeta") || {}).textContent || "";
    return !!document.querySelector("#out .error-box") ||
      (qm !== window.__qmetaBefore && /row\(s\)|triples|boolean|error/i.test(qm));
  }, null, { timeout: 300000 });
  return page.evaluate(() => ({
    rows: document.querySelectorAll("#out table tbody tr").length,
    err: (document.querySelector("#out .error-box") || {}).textContent || "",
    qmeta: (document.getElementById("qmeta") || {}).textContent || "",
    pill: (document.getElementById("sourcePill") || {}).textContent || "",
    facts: window.__reteOpenFacts(),
    blobReads: window.__blobReads.slice(0, 40),
    hash: location.hash,
  }));
};

const main = async () => {
  const browser = await launchBrowser();
  const pageErrors = [];
  let fileBytes = null;
  const report = {};

  try {
    // ---- build the fixture in the page's own engine -------------------------
    const builder = await openPage(browser, { lazyAboveMB: null, asyncReads: false });
    builder.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));
    const built = await builder.evaluate(([n, chunks]) => {
      const B = "https://example.org/gate/local/";
      let seed = 20260102;
      const noise = () => {              // chunks × 6 chars, each a fresh draw
        let s = "";                      // (deriving the second half from the
        for (let k = 0; k < chunks; k++) {   // first would just compress away)
          seed = (seed * 1103515245 + 12345) & 0x7fffffff;
          s += ((seed >>> 8) & 0x7fffff).toString(36).padStart(5, "0") + "-";
        }
        return s;
      };
      const lines = [];
      for (let i = 0; i < n; i++) {
        const s = `<${B}s${String(i).padStart(6, "0")}>`;
        lines.push(`${s} <${B}p/name> "${noise()}" .`);
        lines.push(`${s} <${B}p/kind> <${B}k/${i % 97}> .`);
      }
      const bytes = wasm_bindgen.build(lines.join("\n"), "nt");
      // base64 out: a 5 MB Uint8Array crosses the CDP bridge as a JSON array of
      // 5 million numbers otherwise, which takes longer than the build.
      let bin = "";
      for (let i = 0; i < bytes.length; i += 0x8000) {
        bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      }
      return btoa(bin);
    }, [SUBJECTS, NOISE_CHUNKS]);
    await builder.close();
    fileBytes = Buffer.from(built, "base64");
    report.fileBytes = fileBytes.length;
    t.ok("fixture is multi-block", fileBytes.length > 4 * 1024 * 1024,
      "the in-page build must produce a file many blocks wide, or a byte ratio proves nothing");

    // ---- 1 + 2. lazy, on each engine ---------------------------------------
    for (const [label, asyncReads] of [["async", true], ["sync", false]]) {
      const page = await openPage(browser, { lazyAboveMB: 0, asyncReads });
      page.on("pageerror", (e) => pageErrors.push(`${label}: ${String(e).slice(0, 200)}`));
      const r = await openLocalAndQuery(page, fileBytes);
      const read = (r.facts.lastRead || {});
      const bytes = (read.bytes || 0) + (read.openBytes || 0);
      const requests = (read.requests || 0) + (read.openRequests || 0);
      // Two figures, because they answer different questions: `bytesRead` is
      // this QUERY's cost, `sessionBytes` is everything the session has read —
      // the open, the card, the schema probe and the query. The second is the
      // one that must stay under the file size, or "not read whole" is a claim
      // about one query rather than about opening the file.
      const sessionBytes = read.sessionBytes || 0;
      report[label] = {
        rows: r.rows, pill: r.pill, bytesRead: bytes, requests,
        sessionBytes, sessionRequests: read.sessionRequests || 0,
        fileLength: read.fileLength || 0,
        percentOfFile: read.fileLength ? +(100 * bytes / read.fileLength).toFixed(2) : null,
        sessionPercentOfFile: read.fileLength ? +(100 * sessionBytes / read.fileLength).toFixed(2) : null,
        wholeFileBlobReads: r.blobReads.filter((b) => b.size === fileBytes.length),
        qmeta: r.qmeta.slice(0, 200),
      };

      t.equal(`${label}:rows`, r.rows, 2, "the bounded query must answer over the local file");
      t.equal(`${label}:error`, r.err.slice(0, 160), "", "the local open must not error");
      t.ok(`${label}:lazyMode`, r.facts.local === true, "the file must open through the lazy local reader");
      t.equal(`${label}:noResidentBytes`, r.facts.inMemoryBytes, 0,
        "a lazily-opened file must leave nothing resident in the page");
      t.match(`${label}:pill`, r.pill, /local file \(lazy\)/,
        "the source pill must say local — not remote, and not in-memory");
      // (1) the page itself never turned the File into bytes.
      t.equal(`${label}:wholeFileMaterialized`, report[label].wholeFileBlobReads.length, 0,
        "the page read the whole File into a buffer — the exact #102 regression");
      // (2) the engine read a fraction of it.
      t.ok(`${label}:engineReadFraction`, bytes > 0 && bytes < fileBytes.length / 2,
        `open + one bounded query read ${bytes} of ${fileBytes.length} bytes; a lazy open must read a fraction`);
      t.ok(`${label}:requests`, requests > 0, "a lazy open must issue range reads");
      t.ok(`${label}:sessionReadFraction`, sessionBytes > 0 && sessionBytes < fileBytes.length / 2,
        `the whole session (open + card + schema + query) read ${sessionBytes} of ${fileBytes.length} bytes`);
      // A blob address must never leak into a shareable link.
      t.ok(`${label}:noLocalUrlInHash`, !/rete-local/.test(r.hash),
        `a rete-local: address reached the deep link: ${r.hash.slice(0, 120)}`);
      await page.close();
    }

    // ---- 3. the whole-file fast path survives for small files ---------------
    // Default threshold (128 MiB) against the same multi-MB file: it must still
    // load WHOLE, because that is faster when a query touches everything and it
    // is what lights up the tabs needing the graph resident.
    const small = await openPage(browser, { lazyAboveMB: null, asyncReads: false });
    small.on("pageerror", (e) => pageErrors.push(`default: ${String(e).slice(0, 200)}`));
    const s = await openLocalAndQuery(small, fileBytes);
    report.belowThreshold = {
      rows: s.rows, source: s.facts.source, inMemoryBytes: s.facts.inMemoryBytes,
      lazyAboveBytes: s.facts.lazyAboveBytes,
    };
    t.equal("default:source", s.facts.source, "file",
      "below the threshold a local file must still load whole — the in-memory tabs depend on it");
    t.equal("default:residentBytes", s.facts.inMemoryBytes, fileBytes.length,
      "the whole-file path must hold the whole file");
    t.equal("default:rows", s.rows, 2, "the whole-file path must still answer");
    await small.close();
  } catch (e) {
    t.threw("check_local_lazy", e);
  }

  if (pageErrors.length) t.fail("pageErrors", pageErrors.slice(0, 3).join(" | "));
  await browser.close();
  t.finish({ note: "a local .rete opens through the range reader (both engines), and small files still load whole", ...report }, { indent: 2 });
};

main().catch((e) => {
  t.threw("main", e);
  t.finish({}, { indent: 2 });
});
