// Assemble the .mcpb bundle: one ESM server file, the wasm engine beside it,
// the published-catalogue snapshot, the manifest, and the icon.
//
// The server is bundled with esbuild so the archive carries no node_modules —
// the whole extension is two files of consequence (index.mjs + the .wasm).
// Run through Docker (./build.sh) unless you have node on PATH.
import { build } from "esbuild";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { runInNewContext } from "node:vm";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const out = join(here, "build");
const client = join(repo, "clients", "js");

const wasm = join(client, "dist", "rete_wasm_bg.wasm");
if (!existsSync(wasm)) {
  throw new Error(
    "clients/js/dist is missing — build the JS client first:\n" +
      "  docker compose run --rm --user root dev bash -c 'wasm-pack build crates/rete-wasm " +
      "--target web --no-opt --out-dir /work/clients/js/vendor/pkg'\n" +
      "  docker run --rm -v \"$PWD\":/w -w /w/clients/js node:22-slim node build.mjs",
  );
}

// The catalogue is projected from the playground's catalog.js — the source of
// truth for published datasets — rather than from hf-space/catalog.json, so a
// bundle can never ship a snapshot that has drifted behind it.
const catalogSource = join(repo, "web", "playground-src", "catalog.js");
if (!existsSync(catalogSource)) {
  throw new Error(`${catalogSource} is missing — cannot project the published catalogue`);
}

rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "server"), { recursive: true });

// The extension IS the engine, so it carries the engine's version — stamped
// into the manifest and the server from the workspace Cargo.toml, never
// maintained separately. `mcpb validate` accepts prerelease versions.
const workspace = readFileSync(join(repo, "Cargo.toml"), "utf8");
const version = workspace.match(/\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/)?.[1];
if (!version) throw new Error("could not read the workspace version from Cargo.toml");

await build({
  entryPoints: [join(here, "src", "server.mjs")],
  bundle: true,
  platform: "node",
  target: "node18",
  format: "esm",
  // The client resolves its engine as a sibling of the bundle via import.meta.url;
  // node: builtins stay external (they are provided by the runtime).
  external: ["node:*"],
  alias: { "rete-graph": join(client, "dist", "index.js") },
  define: { __RETE_VERSION__: JSON.stringify(version) },
  outfile: join(out, "server", "index.mjs"),
  banner: { js: `// rete-graphs MCP server v${version} — Apache-2.0 — https://github.com/caviri/rete` },
});

// The engine, loaded at runtime from beside the server bundle.
cpSync(wasm, join(out, "server", "rete_wasm_bg.wasm"));

// The published catalogue: evaluate catalog.js in a sandbox (it assigns to
// window.RETE_PLAYGROUND_CATALOG) and project it onto what the tools serve —
// the same projection scripts/export_space_catalog.py makes for the Space.
const sandbox = { window: {} };
runInNewContext(readFileSync(catalogSource, "utf8"), sandbox, { filename: catalogSource });
const source = sandbox.window.RETE_PLAYGROUND_CATALOG;
if (!source?.datasets?.length) throw new Error("catalog.js defined no datasets");

const base = String(source.remoteBase ?? "").replace(/\/$/, "");
const meta = source.datasetMeta ?? {};
const extra = source.datasetExtra ?? {};
const examples = source.examples ?? {};
const shacl = source.shacl ?? {};

const datasets = source.datasets.map((d) => {
  const m = meta[d.key] ?? {};
  const entry = {
    key: d.key,
    label: d.label,
    description: d.description,
    triples: m.triples,
    size: m.size,
    license: m.license,
    source: m.source,
    tags: (extra[d.key] ?? {}).tags,
    examples: (examples[d.key] ?? [])
      .filter((e) => e.q)
      .map((e) => ({ title: e.label, tip: e.tip, sparql: e.q })),
    shacl_shapes: (shacl[d.key] ?? []).filter((s) => s.shape).map((s) => ({ title: s.label, shape: s.shape })),
  };
  // Sharded datasets have no single file; the server reports their shard URLs
  // rather than pretending one exists.
  if (d.shards?.length) entry.shards = d.shards;
  else entry.url = d.url || `${base}/${d.key}/${d.key}.rete`;
  return Object.fromEntries(
    Object.entries(entry).filter(([, v]) => v !== undefined && v !== null && v !== "" && !(Array.isArray(v) && !v.length)),
  );
});

writeFileSync(join(out, "server", "catalog.json"), JSON.stringify({ remoteBase: base, datasets }));

for (const file of ["icon.png", "README.md"]) {
  if (existsSync(join(here, file))) cpSync(join(here, file), join(out, file));
}

// The manifest travels with the engine's version stamped in.
const manifest = JSON.parse(readFileSync(join(here, "manifest.json"), "utf8"));
writeFileSync(join(out, "manifest.json"), `${JSON.stringify({ ...manifest, version }, null, 2)}\n`);

const mb = (p) => `${(statSync(p).size / 1e6).toFixed(2)} MB`;
console.log(`built ${out} — version ${version} (from the workspace Cargo.toml)`);
console.log(`  server/index.mjs        ${mb(join(out, "server", "index.mjs"))}`);
console.log(`  server/rete_wasm_bg.wasm ${mb(join(out, "server", "rete_wasm_bg.wasm"))}`);
console.log(`  server/catalog.json     ${mb(join(out, "server", "catalog.json"))} (${datasets.length} datasets)`);
console.log("next: npx @anthropic-ai/mcpb pack build rete.mcpb");
