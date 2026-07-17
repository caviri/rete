// We need to forward routine registration from C to Rust
// to avoid the linker removing the static library.
// NOTE: no register_extendr_panic_hook() here — that symbol is extendr-api
// 0.9+; this package pins the 0.8 line (see src/rust/Cargo.toml).

void R_init_rete_extendr(void *dll);

void R_init_rete(void *dll) {
    R_init_rete_extendr(dll);
}
