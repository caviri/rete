// Entry for the p5.js-style single-file builds (dist/rete-graph.js and
// dist/rete-graph.min.js): the wasm engine is embedded (esbuild `binary`
// loader), so one <script src> is the whole client — global `rete`.
import wasmBytes from "../vendor/pkg/rete_wasm_bg.wasm";
import { __setWasmSource } from "./index.js";

__setWasmSource(wasmBytes);

export * from "./index.js";
