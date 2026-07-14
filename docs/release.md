# Rete 1.0.0-rc.1 release candidate

This candidate freezes the intended 1.0 file-format contract, Rust API, CLI,
browser/WASM API, and crates.io package layout for final review. It is a
pre-release: report compatibility problems before 1.0.0 is tagged.

## Install and artifacts

The release provides native archives for:

- Linux GNU: x86-64 and ARM64
- macOS: Intel x86-64 and Apple Silicon ARM64
- Windows MSVC: x86-64 and ARM64

Each archive includes `rete`, Bash/Zsh/Fish/PowerShell completions, `rete(1)`,
README, license, and changelog. `SHA256SUMS` covers every archive and the WASM
bundle. A CycloneDX JSON SBOM and GitHub artifact attestations are published with
the release.

The crates are intended to appear as
[`rete-core`](https://crates.io/crates/rete-core),
[`rete-cli`](https://crates.io/crates/rete-cli), and
[`rete-wasm`](https://crates.io/crates/rete-wasm). The first RC uses the audited
bootstrap procedure; subsequent releases use crates.io trusted publishing with
GitHub OIDC.

## Compatibility statement

Pre-1.0 `.rete` bytes were explicitly unstable. Rebuild every dataset from its
source RDF with the 1.0 toolchain; do not rename an older file and assume it is
compatible. RC-produced files may also need one final rebuild if review changes
the format before 1.0.0. The durable compatibility promise starts at final
1.0.0.

The Rust crates declare Rust 1.87 as their minimum supported version. Public
Rust, CLI, and WASM surfaces follow semantic versioning after final 1.0.0.

## Verify a download

```sh
sha256sum -c SHA256SUMS
gh attestation verify rete-1.0.0-rc.1-x86_64-unknown-linux-gnu.tar.gz \
  --repo caviri/rete
./rete-1.0.0-rc.1-x86_64-unknown-linux-gnu/rete --version
```

Then build and query a source graph:

```sh
rete build source.nq -o graph.rete --pyramid-algo types --card
rete verify graph.rete
rete sparql graph.rete 'SELECT * WHERE { ?s ?p ?o } LIMIT 1'
```

## Known limitations

- Browser execution is single-threaded by default. Threads are experimental and
  require cross-origin isolation.
- Range reads are lazy; query result materialization is currently eager.
- Federation unions results from independent files and does not resolve general
  joins spanning files.
- The RDF/XML parser is temporarily pinned through upstream Oxigraph crates to
  `quick-xml 0.37.5`. The two active RustSec findings are visible in development
  CI, and crates.io publication remains blocked until the release preflight is
  explicitly clean.

Full changes are in the [changelog](https://github.com/caviri/rete/blob/release-1.0.0-rc1/CHANGELOG.md).
