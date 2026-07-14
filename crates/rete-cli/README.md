# rete-cli

`rete-cli` installs the `rete` command for building, inspecting, verifying, and
querying immutable `.rete` RDF graph files.

This is a release candidate. Install the exact version while testing 1.0:

```sh
cargo install rete-cli --version 1.0.0-rc.1 --locked
```

## Build and query

```sh
rete build graph.ttl -o graph.rete --pyramid-algo types --card
rete verify graph.rete
rete sparql graph.rete "SELECT ?s ?o WHERE { ?s <urn:knows> ?o } LIMIT 20"
```

Query a hosted file without downloading it first:

```sh
rete sparql-url https://example.net/graph.rete \
  "SELECT * WHERE { ?s ?p ?o } LIMIT 20"
```

The remote commands use HTTP byte ranges. The host must return exact ranges and
must expose `Content-Range` to browsers if the same file is used by
`rete-wasm`.

For source builds, use the repository's Docker Compose/devcontainer toolchain;
it pins Rust, Clippy, rustfmt, the WASM target, wasm-pack, Python, and uv.

See the [CLI reference](https://caviri.github.io/rete/cli.html), the
[format compatibility policy](https://caviri.github.io/rete/compatibility.html),
and [docs.rs](https://docs.rs/rete-cli).

## License

Apache-2.0. See `LICENSE`.
