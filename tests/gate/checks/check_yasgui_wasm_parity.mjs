// docs/yasgui.html must carry the engine of the build it ships with: the
// wasm-bindgen glue and the .wasm it inlines are web/pkg-nomodules' own, and
// its stamp is the reproducible one build_wasm.sh passes (the workspace
// version), never a date or a local git lookup. Up to 0.4.2 the page had no
// producer in build_wasm.sh and sat on an engine from 2026-08-08 ("Built
// 2026-08-08 from unknown.", no partition_plan) while every release moved on.
//
//   node tests/gate/checks/check_yasgui_wasm_parity.mjs
//
// CI's wasm job byte-diffs the page after its own rebuild; this is the same
// promise checked against the local web/pkg-nomodules the gate runs on.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT
  || path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const t = expect("check_yasgui_wasm_parity");

const html = fs.readFileSync(`${root}/docs/yasgui.html`, "utf8");
const glue = fs.readFileSync(`${root}/web/pkg-nomodules/rete_wasm.js`, "utf8").trimEnd();
const wasm = fs.readFileSync(`${root}/web/pkg-nomodules/rete_wasm_bg.wasm`).toString("base64");
const cargo = fs.readFileSync(`${root}/Cargo.toml`, "utf8");
const version = cargo.match(/^\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m)?.[1];

function inlineScript(id) {
  const open = `<script id="${id}">\n`;
  const at = html.indexOf(open);
  if (at < 0) return null;
  const body = at + open.length;
  const end = html.indexOf("\n</script>", body);
  return end < 0 ? null : html.slice(body, end);
}

const embeddedGlue = inlineScript("reteGlue");
t.ok("gluePresent", embeddedGlue !== null, "docs/yasgui.html has no reteGlue script");
if (embeddedGlue !== null) {
  t.ok("glueCurrent", embeddedGlue === glue,
    "docs/yasgui.html embeds other wasm-bindgen glue than web/pkg-nomodules; rerun scripts/build_wasm.sh");
}
const b64 = html.match(/const RETE_WASM_B64 = "([A-Za-z0-9+/=]*)";/)?.[1];
t.ok("wasmPresent", Boolean(b64), "docs/yasgui.html has no RETE_WASM_B64");
if (b64) {
  t.ok("wasmCurrent", b64 === wasm,
    "docs/yasgui.html embeds another engine than web/pkg-nomodules; rerun scripts/build_wasm.sh");
}
t.ok("versionPresent", Boolean(version), "Cargo.toml has no [workspace.package] version");
if (version) {
  t.ok("reproducibleStamp", html.includes(`const BUILD_STAMP = "Built ${version}.";`),
    `docs/yasgui.html must be stamped "Built ${version}." (RETE_BUILD_STAMP), not a date`);
}

t.finish({ artifact: "docs/yasgui.html", wasmBytes: Buffer.from(wasm, "base64").length });
