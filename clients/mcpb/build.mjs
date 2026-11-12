// Assemble the .mcpb bundle: one ESM server file, the wasm engine beside it,
// the published-catalogue snapshot, the manifest, and the icon.
//
// The server is bundled with esbuild so the archive carries no node_modules —
// the whole extension is two files of consequence (index.mjs + the .wasm).
// Run through Docker (./build.sh) unless you have node on PATH.
import { build } from "esbuild";
import { execFileSync } from "node:child_process";
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
      "--target web --no-opt --out-dir /work/clients/js/vendor/pkg -- --features unicode-regex'\n" +
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

// Which build this is, for the about tool and the view's About panel: the
// commit (RETE_BUILD_COMMIT, CI's GITHUB_SHA, or git — a copy without either
// says it does not know), the date (SOURCE_DATE_EPOCH when set), and
// RETE_BUILD_LABEL, which names a build that is not a release.
function commitOf() {
  const given = process.env.RETE_BUILD_COMMIT || process.env.GITHUB_SHA;
  if (given) return given.slice(0, 12);
  try {
    return execFileSync("git", ["rev-parse", "--short=12", "HEAD"], { cwd: repo, stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
  } catch {
    return null;
  }
}
const builtAt = process.env.SOURCE_DATE_EPOCH ? new Date(Number(process.env.SOURCE_DATE_EPOCH) * 1000) : new Date();

// Who made it and how to cite it, from CITATION.cff — the one place that says.
const cff = readFileSync(join(repo, "CITATION.cff"), "utf8");
const cffField = (key) => cff.match(new RegExp(`^\\s*-?\\s*${key}:\\s*"?([^"\\n]+)"?`, "m"))?.[1]?.trim() ?? null;

// What the bundle carries of other people's work, each with its licence — read
// from the packages themselves, so the list cannot drift from what is inlined.
const BUNDLED = [
  ["@modelcontextprotocol/sdk", "the MCP server"],
  ["@modelcontextprotocol/ext-apps", "MCP Apps: the graph view in the conversation"],
  ["zod", "tool input schemas"],
  ["cytoscape", "drawing the graph view"],
  ["@dagrejs/dagre", "the layered layout"],
  ["@dagrejs/graphlib", "the layered layout"],
];
function packageOf(name) {
  const at = [join(here, "node_modules", name), ...BUNDLED.map(([p]) => join(here, "node_modules", p, "node_modules", name))]
    .map((dir) => join(dir, "package.json"))
    .find(existsSync);
  if (!at) throw new Error(`${name} is not installed — run npm ci first`);
  return JSON.parse(readFileSync(at, "utf8"));
}
const repoUrl = (pkg) => {
  const url = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
  return (pkg.homepage || url || "").replace(/^git\+/, "").replace(/\.git$/, "") || null;
};
const about = {
  name: "rete-graphs",
  title: "rete — knowledge graphs",
  version,
  build: { commit: commitOf(), date: builtAt.toISOString().slice(0, 10), label: process.env.RETE_BUILD_LABEL || null },
  license: "Apache-2.0",
  author: {
    name: [cffField("given-names"), cffField("family-names")].filter(Boolean).join(" "),
    orcid: cffField("orcid"),
  },
  cite: { title: cffField("title"), doi: cffField("doi"), url: cffField("doi") ? `https://doi.org/${cffField("doi")}` : null },
  links: {
    documentation: "https://caviri.github.io/rete/",
    playground: "https://caviri.github.io/rete/playground.html",
    repository: "https://github.com/caviri/rete",
    issues: "https://github.com/caviri/rete/issues",
    releases: "https://github.com/caviri/rete/releases",
    latest_bundle: "https://github.com/caviri/rete/releases/latest/download/rete.mcpb",
  },
  credits: [
    ...BUNDLED.map(([name, role]) => {
      const pkg = packageOf(name);
      return { name, version: pkg.version, license: pkg.license, role, url: repoUrl(pkg) };
    }),
    {
      name: "oxrdf, oxttl, spargebra (Oxigraph)",
      license: "MIT OR Apache-2.0",
      role: "RDF terms, Turtle and N-Triples parsing, SPARQL parsing — inside the engine",
      url: "https://github.com/oxigraph/oxigraph",
    },
  ],
  data: "Every published graph has its own licence and source; dataset_card says which. Credit them when using the data.",
};

// Two bundles side by side: the server, and the query worker it starts on
// worker threads (src/jobs.mjs) — each with its own copy of the client, since
// each thread loads its own engine.
const bundle = {
  bundle: true,
  platform: "node",
  target: "node18",
  format: "esm",
  // The client resolves its engine as a sibling of the bundle via import.meta.url;
  // node: builtins stay external (they are provided by the runtime).
  external: ["node:*"],
  alias: { "rete-graph": join(client, "dist", "index.js") },
  define: { __RETE_VERSION__: JSON.stringify(version), __RETE_ABOUT__: JSON.stringify(about) },
  banner: { js: `// rete-graphs MCP server v${version} — Apache-2.0 — https://github.com/caviri/rete` },
};
await build({ ...bundle, entryPoints: [join(here, "src", "server.mjs")], outfile: join(out, "server", "index.mjs") });
await build({ ...bundle, entryPoints: [join(here, "src", "worker.mjs")], outfile: join(out, "server", "worker.mjs") });

// The graph view (src/view): an MCP App page the host renders in a sandbox that
// runs inline scripts but reaches no network, so its script — cytoscape and the
// MCP Apps client included — and its styles are inlined into one HTML file,
// which src/views.mjs serves as the ui://rete/graph-view.html resource.
const view = await build({
  entryPoints: [join(here, "src", "view", "app.js")],
  bundle: true,
  platform: "browser",
  format: "iife",
  target: "es2020",
  minify: true,
  write: false,
  legalComments: "eof",
  define: { __RETE_ABOUT__: JSON.stringify(about) },
});
const viewHtml = readFileSync(join(here, "src", "view", "index.html"), "utf8")
  .replace("/*STYLE*/", () => readFileSync(join(here, "src", "view", "style.css"), "utf8"))
  .replace("/*SCRIPT*/", () => view.outputFiles[0].text.replaceAll("</script", "<\\/script"));
writeFileSync(join(out, "server", "graph-view.html"), viewHtml);

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
    textIndex: d.textIndex || undefined, // built with --text-index: full-text search
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
console.log(`built ${out} — version ${version} (from the workspace Cargo.toml), build ${about.build.commit ?? "of unknown commit"} ${about.build.date}${about.build.label ? ` "${about.build.label}"` : ""}`);
console.log(`  server/index.mjs        ${mb(join(out, "server", "index.mjs"))}`);
console.log(`  server/worker.mjs       ${mb(join(out, "server", "worker.mjs"))}`);
console.log(`  server/rete_wasm_bg.wasm ${mb(join(out, "server", "rete_wasm_bg.wasm"))}`);
console.log(`  server/catalog.json     ${mb(join(out, "server", "catalog.json"))} (${datasets.length} datasets)`);
console.log(`  server/graph-view.html  ${mb(join(out, "server", "graph-view.html"))}`);
console.log("next: npx @anthropic-ai/mcpb pack build rete.mcpb");
