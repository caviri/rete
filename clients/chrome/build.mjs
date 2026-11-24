#!/usr/bin/env node
// Build Retelens, the Chrome extension, into clients/chrome/build/: the folder Chrome loads with
// "Load unpacked". With `--zip <file>` it also packs that folder as the Chrome Web Store zip.
//
// Three inputs, all from the commit being built:
//   - this folder's sources, copied as they are (RUNTIME below);
//   - the graph view, bundled here from lib/graphkit/src with esbuild, and the map libraries from
//     lib/mapkit/src, plus the licence of every package that ends up in a bundle;
//   - the engine: clients/js/dist/rete-graph.min.js, the single-file rete-graph build with the wasm
//     embedded (`bash clients/js/build-wasm.sh && node clients/js/build.mjs`). Never npm's copy: the
//     extension ships the engine of its own commit, like clients/mcpb.
// The version is the workspace version from the root Cargo.toml, as for clients/mcpb.
import { build } from "esbuild";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateRawSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const out = join(here, "build");
const args = process.argv.slice(2);
const zipAt = args.includes("--zip") ? resolve(args[args.indexOf("--zip") + 1]) : null;

function workspaceVersion() {
  const m = readFileSync(join(repo, "Cargo.toml"), "utf8").match(/^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m);
  if (!m) throw new Error("no [workspace.package] version in Cargo.toml");
  return m[1];
}
const version = workspaceVersion();
// Chrome takes one to four dot-separated integers: a prerelease (0.8.0-rc.1) ships as 0.8.0 and
// carries its full name in version_name.
const chromeVersion = version.match(/^\d+\.\d+\.\d+/)[0];

const engine = process.env.RETE_GRAPH_BUNDLE || join(repo, "clients", "js", "dist", "rete-graph.min.js");
if (!existsSync(engine)) {
  console.error(`no engine at ${engine}\nbuild it first: bash clients/js/build-wasm.sh && node clients/js/build.mjs`);
  process.exit(1);
}

const walk = (dir) => readdirSync(dir).flatMap((n) => (statSync(join(dir, n)).isDirectory() ? walk(join(dir, n)).map((f) => join(n, f)) : [n]));

// What the extension loads at run time. Tests, tools, store assets and the graph view's sources stay out.
const RUNTIME = ["background.js", "content", "engine.html", "engine-frame.js", "fonts", "icons", "lib", "popup.html", "popup.js",
  "rete-cache.js", "rete-worker.js", "rete-xhr-shim.js", "shim.css", "viewer.html", "viewer.js", "vendor"];
const GRAPHKIT = join("lib", "graphkit");
const MAPKIT = join("lib", "mapkit");

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const entry of RUNTIME) cpSync(join(here, entry), join(out, entry), { recursive: true, filter: (src) => relative(here, src) !== GRAPHKIT && relative(here, src) !== MAPKIT });

// The engine label the status line shows ("rete-graph 0.8.0").
for (const f of walk(out).filter((f) => f.endsWith(".js"))) {
  const p = join(out, f), s = readFileSync(p, "utf8");
  if (s.includes("__RETE_VERSION__")) writeFileSync(p, s.replaceAll("__RETE_VERSION__", version));
}

const manifest = JSON.parse(readFileSync(join(here, "manifest.json"), "utf8"));
manifest.version = chromeVersion;
if (version !== chromeVersion) manifest.version_name = version;
writeFileSync(join(out, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

cpSync(engine, join(out, "vendor", "rete-graph.min.js"));
cpSync(join(repo, "LICENSE"), join(out, "LICENSE"));

// The graph view: one IIFE exposing window.GraphKit, with its layout worker embedded as text (Blob worker)
// and also shipped as a file (the extension pages load it by URL). ASCII output: Chrome refuses a content
// script that is not valid UTF-8, and a raw U+FFFF from a minifier once made it refuse the whole extension.
const src = join(here, GRAPHKIT, "src");
const common = { bundle: true, minify: true, format: "iife", target: "chrome110", legalComments: "none", charset: "ascii", metafile: true, write: false, logLevel: "warning" };
const worker = await build({ ...common, entryPoints: [join(src, "worker.js")] });
const workerJs = worker.outputFiles[0].text;
const kit = await build({
  ...common,
  entryPoints: [join(src, "index.js")],
  loader: { ".css": "text" },
  plugins: [{
    name: "worker-text",
    setup(b) {
      b.onResolve({ filter: /^\.\/worker\.built\.txt$/ }, () => ({ path: "worker.built.txt", namespace: "gk-worker" }));
      b.onLoad({ filter: /.*/, namespace: "gk-worker" }, () => ({ contents: workerJs, loader: "text" }));
    },
  }],
});
mkdirSync(join(out, GRAPHKIT), { recursive: true });
writeFileSync(join(out, GRAPHKIT, "graphkit-worker.js"), workerJs);
writeFileSync(join(out, GRAPHKIT, "graphkit.js"), kit.outputFiles[0].text);

// Maps: Leaflet, protomaps-leaflet and pmtiles in one IIFE (window.RetelensMapKit, Leaflet's CSS as text), loaded
// when a map is first opened (lib/maps.js): injected into a host page like graphkit.js, a script tag in the viewer.
const mapkit = await build({ ...common, entryPoints: [join(here, MAPKIT, "src", "index.js")], loader: { ".css": "text" } });
mkdirSync(join(out, MAPKIT), { recursive: true });
writeFileSync(join(out, MAPKIT, "mapkit.js"), mapkit.outputFiles[0].text);

// Licences of exactly the packages esbuild put in each bundle: vendor/graphkit/ and vendor/mapkit/.
function licences(results, dirName) {
  const pkgs = new Map();
  for (const r of results) {
    for (const input of Object.keys(r.metafile.inputs)) {
      const p = input.replace(/\\/g, "/");
      const m = p.match(/^(.*?node_modules\/((?:@[^/]+\/)?[^/]+))\//);
      if (m) pkgs.set(m[2], resolve(m[1]));
    }
  }
  mkdirSync(join(out, "vendor", dirName), { recursive: true });
  for (const [name, dir] of [...pkgs].sort()) {
    const pj = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const lic = readdirSync(dir).find((f) => /^licen[cs]e/i.test(f));
    const head = `${name} ${pj.version} ${pj.license || ""}`.trim();
    const body = lic ? readFileSync(join(dir, lic), "utf8") : "(no licence file shipped; declared in package.json)\n";
    writeFileSync(join(out, "vendor", dirName, `${name.replace("/", "__")}.LICENSE`), `${head}\n\n${body}`);
  }
  return pkgs;
}
const pkgs = licences([worker, kit], "graphkit");
const mapPkgs = licences([mapkit], "mapkit");

// Chrome refuses an injected script (a content script, or a file passed to scripting.executeScript)
// that is not valid UTF-8 or that holds a Unicode noncharacter (U+FDD0..U+FDEF, U+xFFFE, U+xFFFF),
// and the page says only "It isn't UTF-8 encoded". esbuild's charset: "ascii" escapes strings but
// leaves regex literals as written, so check what was built instead of trusting the option.
const utf8 = new TextDecoder("utf-8", { fatal: true });
for (const f of walk(out).filter((f) => f.endsWith(".js"))) {
  let text;
  try { text = utf8.decode(readFileSync(join(out, f))); } catch { throw new Error(`${f}: not valid UTF-8; Chrome will refuse to inject it`); }
  for (const ch of text) {
    const c = ch.codePointAt(0);
    if ((c >= 0xfdd0 && c <= 0xfdef) || (c & 0xfffe) === 0xfffe) {
      throw new Error(`${f}: holds the noncharacter U+${c.toString(16).toUpperCase()}; Chrome will refuse to inject it (write it as an escape)`);
    }
  }
}

const sha = (p) => createHash("sha256").update(readFileSync(p)).digest("hex").slice(0, 16);
console.log(`built ${relative(repo, out)}: Retelens ${version} (manifest ${chromeVersion}), engine ${relative(repo, engine)} sha256 ${sha(engine)}…, graph view with ${pkgs.size} packages, maps with ${mapPkgs.size} (${(Buffer.byteLength(mapkit.outputFiles[0].text) / 1e3).toFixed(0)} kB)`);

if (zipAt) {
  zip(out, zipAt);
  console.log(`packed ${zipAt} (${(statSync(zipAt).size / 1e6).toFixed(2)} MB)`);
}

// A plain zip (deflate, sorted entries, one fixed date), so the same tree always gives the same bytes.
function zip(dir, file) {
  const files = walk(dir).sort();
  const DATE = ((2026 - 1980) << 9) | (1 << 5) | 1;
  const parts = [], central = [];
  let offset = 0;
  for (const rel of files) {
    const data = readFileSync(join(dir, rel));
    const name = Buffer.from(rel.split(sep).join("/"), "utf8");
    const body = deflateRawSync(data, { level: 9 });
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0); local.writeUInt16LE(20, 4); local.writeUInt16LE(0x0800, 6); local.writeUInt16LE(8, 8);
    local.writeUInt16LE(0, 10); local.writeUInt16LE(DATE, 12); local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18); local.writeUInt32LE(data.length, 22); local.writeUInt16LE(name.length, 26); local.writeUInt16LE(0, 28);
    parts.push(local, name, body);
    const c = Buffer.alloc(46);
    c.writeUInt32LE(0x02014b50, 0); c.writeUInt16LE(0x0314, 4); c.writeUInt16LE(20, 6); c.writeUInt16LE(0x0800, 8); c.writeUInt16LE(8, 10);
    c.writeUInt16LE(0, 12); c.writeUInt16LE(DATE, 14); c.writeUInt32LE(crc, 16); c.writeUInt32LE(body.length, 20); c.writeUInt32LE(data.length, 24);
    c.writeUInt16LE(name.length, 28); c.writeUInt32LE(0o100644 * 0x10000, 38); c.writeUInt32LE(offset, 42);
    central.push(c, name);
    offset += 30 + name.length + body.length;
  }
  const cd = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(files.length, 8); end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(cd.length, 12); end.writeUInt32LE(offset, 16);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, Buffer.concat([...parts, cd, end]));
}
