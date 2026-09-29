//! Refuse a wasm build of this crate that did not select getrandom 0.3's
//! `custom` backend.
//!
//! getrandom 0.3 (reached through rete-core's `rdf12-turtle`) has no backend on
//! `wasm32-unknown-unknown` unless one is chosen with a `--cfg`. Without it the
//! build fails inside getrandom, with a message that recommends the `wasm_js`
//! feature, and following that advice would link wasm-bindgen glue Chicory
//! cannot load. So say the right thing here, first. The flag is scoped to the
//! wasm target, so it never reaches a native build:
//!
//!   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend="custom"'
//!
//! (set in clients/java/Dockerfile and .github/workflows/java-test.yml).
fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.starts_with("wasm32-") {
        return;
    }
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    let selected = flags
        .split('\u{1f}')
        .any(|f| f.replace(' ', "").contains("getrandom_backend=\"custom\""));
    if !selected {
        panic!(
            "rete-ffi's wasm build must select getrandom 0.3's custom backend: set \
             CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend=\"custom\"'. \
             Do NOT enable getrandom's `wasm_js`: it links wasm-bindgen glue Chicory cannot load."
        );
    }
}
