// Build the npm package from a FRESH wasm engine built out of the checked-out
// crates (vendor/pkg — produced by ./build-wasm.sh, which runs wasm-pack).
// This deliberately does NOT reuse the committed web/pkg playground
// artifacts: those follow their own build pipeline and can lag the sources.
// Outputs:
//   dist/index.js            ESM entry (wasm as a sibling file, lazy-loaded)
//   dist/rete_wasm_bg.wasm   the engine
//   dist/rete-graph.js       p5.js-style single-file script-tag build
//   dist/rete-graph.min.js   ... minified (wasm embedded in both)
import { build } from "esbuild";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const vendor = join(here, "vendor", "pkg");
const dist = join(here, "dist");

if (!existsSync(join(vendor, "rete_wasm_bg.wasm"))) {
  throw new Error(
    "vendor/pkg is missing — run ./build-wasm.sh first (needs Rust + wasm-pack; see README)",
  );
}

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

const { version } = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));
const banner = `/*! rete-graph v${version} | Apache-2.0 | https://github.com/caviri/rete */`;

// The node: builtins live behind dynamic imports that only execute on Node —
// keep them external so browser bundles never try to resolve them.
const external = ["node:*"];

// ESM: glue + wrapper bundled, wasm stays a sibling file loaded on init().
await build({
  entryPoints: [join(here, "src", "index.js")],
  bundle: true,
  format: "esm",
  external,
  outfile: join(dist, "index.js"),
  banner: { js: banner },
});
cpSync(join(vendor, "rete_wasm_bg.wasm"), join(dist, "rete_wasm_bg.wasm"));

// Script-tag builds: one self-contained file, wasm embedded (global `rete`).
for (const [outfile, minify] of [
  ["rete-graph.js", false],
  ["rete-graph.min.js", true],
]) {
  await build({
    entryPoints: [join(here, "src", "browser-global.js")],
    bundle: true,
    format: "iife",
    globalName: "rete",
    loader: { ".wasm": "binary" },
    external,
    minify,
    outfile: join(dist, outfile),
    banner: { js: banner },
  });
}

console.log("built dist/ for rete-graph", version);
