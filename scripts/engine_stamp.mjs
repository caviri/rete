#!/usr/bin/env node
// Which engine source is a built wasm package made from?
//
// web/pkg* is gitignored build output, so nothing in git says which source it
// came from — and an agent once ran the gate against a web/pkg built from a
// different engine commit (d9892711) than the one it was testing, which a green
// gate then vouched for. This file is the one answer to "does this package
// match this checkout?", shared by the producer and the checker:
//
//   node scripts/engine_stamp.mjs hash                 # the current engine source
//   node scripts/engine_stamp.mjs stamp web/pkg …      # write <dir>/rete-build.json
//   node scripts/engine_stamp.mjs check [--json]       # every package vs this checkout
//
// scripts/build_wasm.sh (and build_playground_async.sh) stamp every package
// they build; the gate's G0 runs `check` and fails on a stale or unstamped one.
//
// The ENGINE SOURCE is a content hash, not a commit: a commit says nothing about
// uncommitted edits, and a git worktree's .git cannot be read from inside the
// container that builds the wasm. It covers exactly what the wasm is compiled
// from — the workspace Cargo.toml, Cargo.lock, rust-toolchain.toml, and the
// Cargo.toml, build.rs and src/ of rete-wasm and of every crate it reaches by
// `path =` dependency — so a comment-only edit counts (rustc bakes line numbers
// into panic locations, see AGENTS.md) and a playground-only edit does not.
// Node built-ins only: it runs in the dev image and in the Playwright image.
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const STAMP = "rete-build.json";
// The packages scripts/build_wasm.sh produces, and the command that rebuilds
// each (RETE_SOURCE_REVISION is what a git worktree needs, see AGENTS.md).
const REBUILD = (svc) => `docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) ${svc}`;
export const PACKAGES = [
  { dir: "web/pkg", rebuild: REBUILD("wasm") },
  { dir: "web/pkg-nomodules", rebuild: REBUILD("wasm") },
  { dir: "web/pkg-nomodules-async", rebuild: REBUILD("wasm-async") },
];

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
const posix = (p) => p.split(path.sep).join("/");

// `path = "…"` dependencies of one Cargo.toml, from the tables that feed a
// build ([dependencies], [build-dependencies], their [target.….] forms and the
// [dependencies.<name>] spelling). dev-dependencies are not compiled into the
// wasm, so they are not inputs.
function pathDeps(manifest) {
  const out = [];
  let table = "";
  for (const raw of fs.readFileSync(manifest, "utf8").split("\n")) {
    const line = raw.replace(/#.*$/, "").trim();
    const header = /^\[([^\]]+)\]$/.exec(line);
    if (header) { table = header[1].trim(); continue; }
    if (!/(^|\.)(dependencies|build-dependencies)(\.[^.]+)?$/.test(table)) continue;
    if (/(^|\.)dev-dependencies/.test(table)) continue;
    const m = /\bpath\s*=\s*"([^"]+)"/.exec(line);
    if (m) out.push(path.resolve(path.dirname(manifest), m[1]));
  }
  return out;
}

function walk(dir, acc) {
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, ent.name);
    if (ent.isDirectory()) walk(p, acc);
    else if (ent.isFile()) acc.push(p);
  }
  return acc;
}

// The engine source: { sha256, files, crates }.
export function engineSource(root = ROOT) {
  const crates = [];
  const todo = [path.join(root, "crates/rete-wasm")];
  while (todo.length) {
    const c = todo.pop();
    if (crates.includes(c)) continue;
    const manifest = path.join(c, "Cargo.toml");
    if (!fs.existsSync(manifest)) throw new Error(`no Cargo.toml in ${c}`);
    crates.push(c);
    todo.push(...pathDeps(manifest));
  }
  const files = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"]
    .map((f) => path.join(root, f))
    .filter((f) => fs.existsSync(f));
  for (const c of crates) {
    for (const f of ["Cargo.toml", "build.rs"]) if (fs.existsSync(path.join(c, f))) files.push(path.join(c, f));
    if (fs.existsSync(path.join(c, "src"))) walk(path.join(c, "src"), files);
  }
  const rel = files.map((f) => posix(path.relative(root, f))).sort();
  const h = createHash("sha256");
  for (const r of rel) h.update(`${r}\0${sha256(fs.readFileSync(path.join(root, r)))}\n`);
  return {
    sha256: h.digest("hex"),
    files: rel.length,
    crates: crates.map((c) => posix(path.relative(root, c))).sort(),
  };
}

// What the wasm says processed it: wasm-bindgen writes "<version> (<rev>)" into
// the `producers` custom section (tool-conventions ProducersSection), so this is
// the CLI that really ran, not the one we hoped for. null when absent.
export function producedBy(bytes, tool = "wasm-bindgen") {
  let sections;
  try { sections = WebAssembly.Module.customSections(new WebAssembly.Module(bytes), "producers"); }
  catch (_e) { return null; }
  for (const s of sections) {
    const b = new Uint8Array(s);
    let p = 0;
    const uleb = () => { let r = 0, shift = 0, x; do { x = b[p++]; r |= (x & 0x7f) << shift; shift += 7; } while (x & 0x80); return r >>> 0; };
    const str = () => { const n = uleb(); const t = new TextDecoder().decode(b.subarray(p, p + n)); p += n; return t; };
    try {
      for (let f = uleb(); f > 0; f--) {
        const field = str();
        for (let v = uleb(); v > 0; v--) {
          const name = str(), version = str();
          if (field === "processed-by" && name === tool) return version;
        }
      }
    } catch (_e) { return null; }
  }
  return null;
}

function stamp(dirs) {
  const src = engineSource();
  for (const d of dirs) {
    const wasm = path.join(ROOT, d, "rete_wasm_bg.wasm");
    if (!fs.existsSync(wasm)) throw new Error(`${d}: no rete_wasm_bg.wasm to stamp`);
    const bytes = fs.readFileSync(wasm);
    const doc = {
      schemaVersion: 1,
      gitCommit: process.env.RETE_SOURCE_REVISION || null,
      engineSource: src.sha256,
      engineSourceFiles: src.files,
      wasmBindgen: producedBy(bytes),
      wasm: { bytes: bytes.length, sha256: sha256(bytes) },
    };
    fs.writeFileSync(path.join(ROOT, d, STAMP), JSON.stringify(doc, null, 2) + "\n");
    console.log(`>> stamped ${d}/${STAMP}: engine source ${src.sha256.slice(0, 12)} (${src.files} files)`);
  }
}

// Every package against this checkout. A package that is absent is reported
// but is not stale (gate.sh's own preflight says how to build a missing one).
export function check(root = ROOT) {
  const src = engineSource(root);
  const packages = PACKAGES.map(({ dir, rebuild }) => {
    const wasm = path.join(root, dir, "rete_wasm_bg.wasm");
    if (!fs.existsSync(wasm)) return { dir, status: "missing", rebuild };
    const sf = path.join(root, dir, STAMP);
    if (!fs.existsSync(sf)) {
      return { dir, status: "unstamped", rebuild,
        why: `no ${STAMP}: built before stamps existed, or not by scripts/build_wasm.sh` };
    }
    let st;
    try { st = JSON.parse(fs.readFileSync(sf, "utf8")); } catch (e) {
      return { dir, status: "unstamped", rebuild, why: `${STAMP} does not parse: ${e.message}` };
    }
    const actual = sha256(fs.readFileSync(wasm));
    if (st.wasm && st.wasm.sha256 !== actual) {
      return { dir, status: "stale", rebuild, gitCommit: st.gitCommit,
        why: "rete_wasm_bg.wasm was replaced after it was stamped" };
    }
    if (st.engineSource !== src.sha256) {
      return { dir, status: "stale", rebuild, gitCommit: st.gitCommit,
        why: `built from engine source ${String(st.engineSource).slice(0, 12)}` +
          `${st.gitCommit ? ` (commit ${String(st.gitCommit).slice(0, 12)})` : ""}` +
          `, this checkout is ${src.sha256.slice(0, 12)}` };
    }
    return { dir, status: "current", gitCommit: st.gitCommit };
  });
  // The TRACKED artifacts' manifest, for information: CI's parity job is what
  // enforces that docs/ matches, so a mismatch here is not a failure.
  let committed = null;
  try {
    const m = JSON.parse(fs.readFileSync(path.join(root, "docs/wasm-build.json"), "utf8"));
    committed = { gitCommit: m.gitCommit, engineSource: m.engineSource || null,
      matchesCheckout: m.engineSource ? m.engineSource === src.sha256 : null };
  } catch (_e) { /* no manifest in this checkout */ }
  const bad = packages.filter((p) => p.status === "stale" || p.status === "unstamped");
  return { verdict: bad.length ? "FAIL" : "PASS", engineSource: src.sha256, files: src.files,
    packages, committedArtifacts: committed };
}

function main(argv) {
  const [cmd, ...rest] = argv;
  if (cmd === "hash") {
    console.log(JSON.stringify(engineSource(), null, 2));
    return 0;
  }
  if (cmd === "stamp") {
    if (!rest.length) throw new Error("stamp: name the package directories");
    stamp(rest);
    return 0;
  }
  if (cmd === "check") {
    const r = check();
    if (rest.includes("--json")) { console.log(JSON.stringify(r, null, 2)); return r.verdict === "PASS" ? 0 : 1; }
    console.log(`engine source of this checkout: ${r.engineSource.slice(0, 12)} (${r.files} files)`);
    for (const p of r.packages) {
      const tag = p.status === "current" ? "ok     " : p.status === "missing" ? "missing" : "STALE  ";
      console.log(`  ${tag} ${p.dir}${p.why ? ` — ${p.why}` : ""}${p.status === "current" && p.gitCommit ? ` (commit ${p.gitCommit.slice(0, 12)})` : ""}`);
      if (p.status === "stale" || p.status === "unstamped") console.log(`          rebuild: ${p.rebuild}`);
    }
    if (r.committedArtifacts) {
      const c = r.committedArtifacts;
      console.log(`  docs/wasm-build.json: commit ${String(c.gitCommit).slice(0, 12)}` +
        (c.matchesCheckout == null ? " (no engineSource recorded)" : c.matchesCheckout ? ", engine source matches" : ", engine source DIFFERS (CI's parity job decides)"));
    }
    return r.verdict === "PASS" ? 0 : 1;
  }
  console.error("usage: node scripts/engine_stamp.mjs hash | stamp <dir>… | check [--json]");
  return 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { process.exit(main(process.argv.slice(2))); } catch (e) { console.error(`engine_stamp: ${e.message}`); process.exit(2); }
}
