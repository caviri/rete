import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { pathToFileURL } from "node:url";
import { TextDecoder, TextEncoder } from "node:util";

const root = process.env.RETE_ROOT || "/work";
const webDir = process.env.RETE_WASM_WEB_DIR || `${root}/web/pkg`;
const noModulesDir = process.env.RETE_WASM_NOMOD_DIR || `${root}/web/pkg-nomodules`;

async function bootWeb(directory) {
  const gluePath = path.resolve(directory, "rete_wasm.js");
  const wasmPath = path.resolve(directory, "rete_wasm_bg.wasm");
  const module = await import(`${pathToFileURL(gluePath).href}?boot-check=${Date.now()}`);
  const bytes = fs.readFileSync(wasmPath);
  await module.default({ module_or_path: bytes });
}

function bootNoModules(directory) {
  const gluePath = path.resolve(directory, "rete_wasm.js");
  const wasmPath = path.resolve(directory, "rete_wasm_bg.wasm");
  const context = vm.createContext({
    console,
    TextDecoder,
    TextEncoder,
    URL,
    WebAssembly,
    Uint8Array,
  });
  vm.runInContext(fs.readFileSync(gluePath, "utf8"), context, {
    filename: gluePath,
  });
  context.wasmBytes = fs.readFileSync(wasmPath);
  vm.runInContext("wasm_bindgen.initSync({ module: wasmBytes })", context);
}

await bootWeb(webDir);
bootNoModules(noModulesDir);

console.log(JSON.stringify({
  verdict: "PASS",
  web: webDir,
  noModules: noModulesDir,
}));
