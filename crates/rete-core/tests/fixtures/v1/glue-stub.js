// Stand-in for web/pkg-nomodules/rete_wasm.js in tests that build a polyglot
// page. `crates/rete-core/tests/polyglot_roundtrip.rs` asserts the builder ↔
// reader marker contract, which has nothing to do with the engine — and the real
// glue is a 120 KB generated, gitignored artifact. Keeping a stub here lets the
// test run from a bare checkout with no wasm build.
var wasm_bindgen = function () {
  throw new Error("stub engine glue — not a real rete-wasm build");
};
