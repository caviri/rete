// tables-duckdb.js — explore a dataset's columnar companion tables (the Parquet
// per-class files behind its DuckDB/SQLite companions) right in the browser with
// DuckDB-Wasm. Like the .rete itself, Parquet is read over HTTP *range*, so a
// query pulls only the row groups it needs. DuckDB-Wasm is heavy, so it's loaded
// lazily (dynamic import from the CDN) only when the user opens the explorer.

let CONN = null; // one shared DuckDB connection per page

const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
const abs = (u) => { try { return new URL(u, location.href).href; } catch (_) { return u; } };

async function initDuckDB() {
  if (CONN) return CONN;
  const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm");
  const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
  const workerUrl = URL.createObjectURL(
    new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" })
  );
  const worker = new Worker(workerUrl);
  const db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), worker);
  await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
  URL.revokeObjectURL(workerUrl);
  CONN = await db.connect();
  return CONN;
}

async function runSql(conn, sql) {
  const res = await conn.query(sql);
  const cols = res.schema.fields.map((f) => f.name);
  const rows = res.toArray().slice(0, 300).map((r) => r.toJSON());
  return { cols, rows };
}

/** Mount the explorer into `host` for a dataset with a Parquet companion. */
export function mountTableExplorer(host, entry, token) {
  const pq = (entry.companions || []).find((c) => c.kind === "parquet");
  if (!pq) { host.innerHTML = ""; return; }
  const dir = abs(pq.url).replace(/\/?$/, "/");
  const remote = /hf\.space|katospiegel/i.test(dir);
  const tk = remote && token ? "?token=" + token : "";
  const fileUrl = (f) => dir + f + tk;
  const manifestUrl = fileUrl("_manifest.parquet");

  host.innerHTML = `
    <div class="notice">Query the columnar companion tables with <b>DuckDB-Wasm</b> — Parquet read over HTTP range, entirely in your browser (same bytes-on-demand idea as the .rete).</div>
    <button class="run" id="tblLoad">Load table explorer</button>
    <div id="tblUI" hidden>
      <div class="notice" style="margin:10px 0 4px">tables (click to query):</div>
      <div class="starter" id="tblFiles"></div>
      <textarea class="sparql" id="tblSql" spellcheck="false"></textarea>
      <div class="run-row"><button class="run" id="tblRun" disabled>Run SQL</button><span class="status" id="tblStatus"></span></div>
      <div class="results" id="tblResults" hidden></div>
    </div>`;

  const $ = (id) => document.getElementById(id);
  const status = (t) => ($("tblStatus").textContent = t);
  const showResults = (r, err) => {
    const box = $("tblResults");
    box.hidden = false;
    if (err) { box.innerHTML = `<div class="warnbox" style="border-radius:10px">${esc(err)}</div>`; return; }
    if (!r.rows.length) { box.innerHTML = `<div style="padding:14px;color:var(--faint)">No rows.</div>`; return; }
    box.innerHTML =
      `<table class="rs"><thead><tr>${r.cols.map((c) => `<th>${esc(c)}</th>`).join("")}</tr></thead><tbody>${r.rows
        .map((row) => `<tr>${r.cols.map((c) => `<td>${cell(row[c])}</td>`).join("")}</tr>`)
        .join("")}</tbody></table>` +
      `<div class="notice" style="padding:8px 12px">${r.rows.length} rows${r.rows.length === 300 ? " (capped)" : ""}</div>`;
  };

  $("tblLoad").onclick = async () => {
    $("tblLoad").disabled = true;
    status("loading DuckDB-Wasm…");
    let conn;
    try { conn = await initDuckDB(); }
    catch (e) { status(""); $("tblLoad").disabled = false; showResults(null, "Couldn't load DuckDB-Wasm (needs network): " + e); $("tblUI").hidden = false; return; }
    $("tblUI").hidden = false;
    $("tblRun").disabled = false;
    $("tblSql").value = `SELECT * FROM read_parquet('${manifestUrl}') LIMIT 100`;

    const run = async () => {
      const sql = $("tblSql").value.trim();
      if (!sql) return;
      $("tblRun").disabled = true;
      status("running…");
      const t0 = performance.now();
      try { showResults(await runSql(conn, sql)); status(((performance.now() - t0) / 1000).toFixed(2) + "s"); }
      catch (e) { showResults(null, String(e)); status("error"); }
      finally { $("tblRun").disabled = false; }
    };
    $("tblRun").onclick = run;

    // Read the manifest, list the per-class tables as clickable buttons.
    try {
      const man = await runSql(conn, `SELECT * FROM read_parquet('${manifestUrl}')`);
      showResults(man);
      const files = [];
      for (const row of man.rows)
        for (const v of Object.values(row))
          if (typeof v === "string" && v.endsWith(".parquet") && !files.includes(v)) files.push(v);
      const fe = $("tblFiles");
      fe.innerHTML = "";
      files.slice(0, 30).forEach((f) => {
        const b = document.createElement("button");
        b.textContent = f.replace(/\.parquet$/, "");
        b.onclick = () => { $("tblSql").value = `SELECT * FROM read_parquet('${fileUrl(f)}') LIMIT 100`; run(); };
        fe.appendChild(b);
      });
      status(files.length ? `ready — ${files.length} tables in the manifest` : "ready");
    } catch (e) {
      showResults(null, String(e));
      status("error");
    }
  };
}

function cell(v) {
  if (v == null) return `<span style="color:var(--faint)">—</span>`;
  if (typeof v === "bigint") return esc(v.toString());
  if (typeof v === "object") return esc(JSON.stringify(v));
  return esc(String(v));
}
