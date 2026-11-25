// What the extension's build (build.mjs) and the file explorer's copy of the
// graph view (build-explorer.mjs) share: the version, the About facts, and the
// graph view page itself — one HTML file with its script and styles inlined.
import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const here = dirname(fileURLToPath(import.meta.url));
export const repo = join(here, "..", "..");

/** The engine's version, from the workspace Cargo.toml: the extension IS the engine. */
export function workspaceVersion() {
  const workspace = readFileSync(join(repo, "Cargo.toml"), "utf8");
  const version = workspace.match(/\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/)?.[1];
  if (!version) throw new Error("could not read the workspace version from Cargo.toml");
  return version;
}

/**
 * Which build this is, for the about tool and the view's About panel: the
 * commit (RETE_BUILD_COMMIT, CI's GITHUB_SHA, or git — a copy without either
 * says it does not know).
 */
export function commitOf() {
  const given = process.env.RETE_BUILD_COMMIT || process.env.GITHUB_SHA;
  if (given) return given.slice(0, 12);
  try {
    return execFileSync("git", ["rev-parse", "--short=12", "HEAD"], { cwd: repo, stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
  } catch {
    return null;
  }
}

// What the bundle carries of other people's work, each with its licence — read
// from the packages themselves, so the list cannot drift from what is inlined.
export const BUNDLED = [
  ["@modelcontextprotocol/sdk", "the MCP server"],
  ["@modelcontextprotocol/ext-apps", "MCP Apps: the graph view in the conversation"],
  ["zod", "tool input schemas"],
  ["cytoscape", "drawing the graph view"],
  ["@dagrejs/dagre", "the layered layout"],
  ["@dagrejs/graphlib", "the layered layout"],
  ["@google/model-viewer", "3D models in the card view"],
  ["three", "3D models in the card view"],
  ["lit", "3D models in the card view"],
  ["@monogrid/gainmap-js", "3D models in the card view"],
  ["leaflet", "the Map tab"],
  ["protomaps-leaflet", "the Map tab: vector tiles drawn on canvas"],
  ["pmtiles", "the Map tab: PMTiles archives read by HTTP Range"],
  ["@mapbox/point-geometry", "the Map tab"],
  ["@mapbox/vector-tile", "the Map tab"],
  ["pbf", "the Map tab"],
  ["potpack", "the Map tab"],
  ["rbush", "the Map tab"],
  ["quickselect", "the Map tab"],
  ["color2k", "the Map tab"],
  ["@protomaps/basemaps", "the Map tab: the basemap's style"],
];

// The result cells, column statistics, query hints and maps of Retelens (clients/chrome/lib): one copy, bundled
// into this page as they are. The map libraries (Leaflet, protomaps-leaflet, pmtiles) ride along as text, like the
// 3D viewer, and start when the Map tab first opens (src/view/app.js: RetelensHost.loadMapKit).
export const RETELENS = join(repo, "clients", "chrome", "lib");

export function packageOf(name) {
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

/**
 * Version, build, author, links and credits, for the about tool and the
 * view's About panel. `build` is { commit, date, label }.
 */
export function aboutOf({ version, build: stamp, bundled = BUNDLED }) {
  // Who made it and how to cite it, from CITATION.cff — the one place that says.
  const cff = readFileSync(join(repo, "CITATION.cff"), "utf8");
  const cffField = (key) => cff.match(new RegExp(`^\\s*-?\\s*${key}:\\s*"?([^"\\n]+)"?`, "m"))?.[1]?.trim() ?? null;
  return {
    name: "rete-graphs",
    title: "rete — knowledge graphs",
    version,
    build: stamp,
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
      ...bundled.map(([name, role]) => {
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
}

/**
 * The graph view (src/view): an MCP App page the host renders in a sandbox that
 * runs inline scripts but reaches no network, so its script — cytoscape and the
 * MCP Apps client included — and its styles are inlined into one HTML file.
 */
export async function buildViewHtml(about) {
  // The 3D viewer rides along as text in a script the page never runs until a
  // card has a model to show (src/view/app.js: loadModelViewer): it is a
  // megabyte, and most views have no model.
  const viewer = await build({
    stdin: { contents: `import "@google/model-viewer";`, resolveDir: here, sourcefile: "model-viewer.js" },
    bundle: true,
    platform: "browser",
    format: "iife",
    target: "es2020",
    minify: true,
    write: false,
    legalComments: "none",
  });
  const mapkit = await build({
    entryPoints: [join(RETELENS, "mapkit", "src", "index.js")],
    bundle: true,
    platform: "browser",
    format: "iife",
    target: "es2020",
    minify: true,
    write: false,
    charset: "ascii",
    legalComments: "none",
    loader: { ".css": "text" },
    nodePaths: [join(here, "node_modules")], // the packages are this package's dependencies (package.json)
  });
  const view = await build({
    entryPoints: [join(here, "src", "view", "app.js")],
    nodePaths: [join(here, "node_modules")],
    bundle: true,
    platform: "browser",
    format: "iife",
    target: "es2020",
    minify: true,
    write: false,
    legalComments: "eof",
    define: { __RETE_ABOUT__: JSON.stringify(about) },
  });
  return readFileSync(join(here, "src", "view", "index.html"), "utf8")
    .replace("/*STYLE*/", () => ["rl-common.css", "rl-rich.css"].map((f) => readFileSync(join(RETELENS, f), "utf8")).join("\n") + "\n" + readFileSync(join(here, "src", "view", "style.css"), "utf8"))
    .replace("/*MAPKIT*/", () => escapeScript(mapkit.outputFiles[0].text))
    .replace("/*MODELVIEWER*/", () => escapeScript(viewer.outputFiles[0].text))
    .replace("/*SCRIPT*/", () => escapeScript(view.outputFiles[0].text));
}

/** Script text that is safe inside a <script> element: no "</script" and no "<!--" (both can only be in strings). */
function escapeScript(text) {
  return text.replaceAll("</script", "<\\/script").replaceAll("<!--", "<\\x21--");
}
