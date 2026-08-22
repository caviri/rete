import fs from "node:fs";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT || "/work";
const htmlPath = `${root}/docs/yasgui.html`;
const gluePath = `${root}/web/pkg-nomodules/rete_wasm.js`;
const t = expect("check_yasgui_wasm_parity");

function embeddedScript(html, id) {
  const open = `<script id="${id}">\n`;
  const start = html.indexOf(open);
  if (start < 0) return null;
  const bodyStart = start + open.length;
  const end = html.indexOf("\n</script>", bodyStart);
  return end < 0 ? null : html.slice(bodyStart, end);
}

const html = fs.readFileSync(htmlPath, "utf8");
const generatedGlue = fs.readFileSync(gluePath, "utf8").trimEnd();
const embeddedGlue = embeddedScript(html, "reteGlue");
const cargo = fs.readFileSync(`${root}/Cargo.toml`, "utf8");
const workspaceVersion = cargo.match(/\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m)?.[1];

t.ok("reteGluePresent", embeddedGlue !== null,
  "docs/yasgui.html is missing the generated reteGlue script");
if (embeddedGlue !== null) {
  t.equal("reteGlueCurrent", embeddedGlue, generatedGlue,
    "docs/yasgui.html embeds stale wasm-bindgen glue; run scripts/build_wasm.sh");
}
t.ok("workspaceVersionPresent", Boolean(workspaceVersion),
  "Cargo.toml is missing [workspace.package].version");
if (workspaceVersion) {
  t.ok("deterministicBuildStamp",
    html.includes(`const BUILD_STAMP = "Built ${workspaceVersion}.";`),
    "docs/yasgui.html must use RETE_BUILD_STAMP, not a build date or worktree-local git result");
}

t.finish({ artifact: "docs/yasgui.html" });
