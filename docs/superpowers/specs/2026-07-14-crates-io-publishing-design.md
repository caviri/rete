# crates.io Publishing Design for Rete 1.0

**Status:** Approved design, pending implementation-plan integration

**Date:** 2026-07-14

**Branch:** `release-1.0.0-rc1`

## Decision

Publish `rete-core`, `rete-cli`, and `rete-wasm` on crates.io as part of the Rete 1.0 release process.

The first publication, `1.0.0-rc.1`, is bootstrapped manually from the checked-in devcontainer/Compose environment with an ephemeral crates.io token. After those crate records exist, each crate is configured to trust the repository's GitHub Actions release workflow. Every later RC and the final `1.0.0` release use crates.io Trusted Publishing with short-lived OIDC credentials and no stored registry secret.

## Goals

- Publish all three public crates with complete metadata and registry-resolvable dependencies.
- Make the initial publication auditable and reproducible without using host Rust tooling.
- Avoid storing a crates.io API token in GitHub, the repository, container volumes, shell history, or Cargo credential files.
- Publish dependent crates only after their exact `rete-core` version is visible in the crates.io sparse index.
- Require the same security, compatibility, API, package, and test gates used for GitHub release artifacts.
- Make retries safe after a partial publication without attempting to overwrite immutable crate versions.
- Use a protected GitHub environment and a manual approval gate for every OIDC-backed publication.

## Non-goals

- The first publish does not use Trusted Publishing; crates.io requires a crate to exist before its trusted publisher can be configured.
- The workflow does not publish from pull requests, ordinary branch pushes, or unprotected `workflow_dispatch` runs.
- The workflow does not publish `docgen` or `rete-bench`.
- This design does not publish an npm package. The stable WASM interface is delivered by the `rete-wasm` crate and the release's generated WASM artifacts.
- Published crate versions are never deleted or overwritten. A faulty release is yanked and replaced by a higher version.

## Published Crates and Dependency Order

| Order | Package | Registry dependency | Product role |
|---|---|---|---|
| 1 | `rete-core` | none within this workspace | Stable format, range reader, query, SHACL, and reasoning API |
| 2 | `rete-cli` | `rete-core = "=1.0.0-rc.1"` | `rete` command-line executable |
| 3 | `rete-wasm` | `rete-core = "=1.0.0-rc.1"` | Browser/WASM bindings |

The exact internal dependency changes to `=1.0.0` for the final release. Path and version are specified together in the workspace so local builds use the worktree while packaged crates resolve through crates.io.

Each public manifest sets:

```toml
publish = ["crates-io"]
repository.workspace = true
homepage.workspace = true
rust-version.workspace = true
```

Each package includes a crate-local README and LICENSE, an Apache-2.0 license declaration, description, documentation link, keywords, categories, and an explicit include list. `cargo package --list` is reviewed before the bootstrap.

## Phase 1: One-time Manual Bootstrap

### Preconditions

Before publishing `1.0.0-rc.1`:

1. The release worktree is clean and checked out at the signed `v1.0.0-rc.1` tag.
2. The tag commit has passed every required release check.
3. `cargo publish --dry-run -p rete-core` passes in the devcontainer.
4. `cargo package -p rete-cli --no-verify` and `cargo package -p rete-wasm --no-verify` pass; full dependent verification waits for the core crate to become registry-visible.
5. `rete-core`, `rete-cli`, and `rete-wasm` remain unclaimed on crates.io.
6. The publisher's crates.io account has a verified email and is authenticated through the intended GitHub identity.
7. A crates.io API token is created with permission to publish new crates, the shortest available expiry not exceeding 24 hours, and no unrelated crate scope.

### Token handling

The bootstrap token exists only as `CARGO_REGISTRY_TOKEN` in the interactive devcontainer process:

```sh
read -r -s -p "crates.io bootstrap token: " CARGO_REGISTRY_TOKEN
printf '\n'
export CARGO_REGISTRY_TOKEN
bash scripts/publish_crates.sh --bootstrap 1.0.0-rc.1
unset CARGO_REGISTRY_TOKEN
```

The process must not run `cargo login`. The token is not passed as a command-line argument, printed under `set -x`, saved in `.env`, written to `$CARGO_HOME/credentials.toml`, or mounted into another container. The token is revoked immediately after all three packages are verified.

### Bootstrap publication algorithm

`scripts/publish_crates.sh --bootstrap 1.0.0-rc.1` performs these operations:

1. Require a clean worktree at tag `v1.0.0-rc.1`.
2. Read all three versions through `cargo metadata --no-deps`; require exact equality with `1.0.0-rc.1`.
3. Require the CLI and WASM manifests to depend on `rete-core = "=1.0.0-rc.1"`.
4. Re-run package, security, rustdoc, format-compatibility, and SemVer-surface checks.
5. Package `rete-core`, record its `.crate` SHA-256, and run `cargo publish --dry-run -p rete-core`.
6. Publish `rete-core` with `cargo publish -p rete-core --locked`.
7. Poll the crates.io API and sparse index for `rete-core 1.0.0-rc.1` for up to ten minutes.
8. Run `cargo publish --dry-run -p rete-cli`, publish it with `--locked`, and poll for registry visibility.
9. Run `cargo publish --dry-run -p rete-wasm`, publish it with `--locked`, and poll for registry visibility.
10. Download all three registry `.crate` archives and require their SHA-256 checksums to match the locally recorded packages.
11. Verify the crates.io metadata, repository link, README rendering, license, features, dependencies, and owners.
12. Poll docs.rs separately. A docs.rs failure blocks release completion but does not cause a duplicate publication attempt; use the crates.io docs rebuild control after correcting documentation configuration.

The script writes a non-secret JSON receipt to `target/release/crates-io-receipt.json` containing package names, versions, checksums, registry URLs, Git commit, tag, and timestamps. The receipt is attached to the GitHub prerelease.

## Partial Failure and Retry Safety

crates.io versions are immutable, so retry behavior is explicit:

- If the exact package version is absent, publish it.
- If the exact version exists, download it and compare its checksum with the locally packaged `.crate` from the same clean tag.
- If the checksums match, mark that package complete and continue with the next dependency.
- If the checksums differ, stop immediately. Do not publish either dependent crate and do not treat the release as successful.
- If publication succeeds but index visibility exceeds ten minutes, stop before publishing dependents. A rerun resumes after verifying the existing checksum.
- If `rete-core` exists but `rete-cli` or `rete-wasm` fails, a rerun verifies core and resumes at the first missing dependent.
- If a published package is semantically faulty, yank it and publish a higher RC. Never attempt to reuse its version.

## Phase 2: Configure Trusted Publishing

After the three `1.0.0-rc.1` packages exist, configure a trusted publisher on crates.io for each crate with exactly:

- GitHub owner: `caviri`
- Repository: `rete`
- Workflow file: `release.yml`
- GitHub environment: `crates-io`

Configure the GitHub `crates-io` environment with:

- Required manual approval.
- Deployment branch/tag rules restricted to protected version tags matching `v*` from the canonical repository.
- No `CARGO_REGISTRY_TOKEN` secret.
- No permission for pull-request workflows or forks to access the environment.

Run a manual authentication-only verification after configuration. The protected job requests a short-lived token through `rust-lang/crates-io-auth-action@v1` but performs no publication. Successful token exchange proves that repository, workflow filename, and environment match the crates.io trusted-publisher records.

## Phase 3: GitHub Actions Publication

The existing `.github/workflows/release.yml` owns OIDC publication so the trusted-publisher filename is stable. Its publish job runs only after every build, test, security, package, compatibility, artifact, and browser job succeeds.

The job has minimum permissions:

```yaml
permissions:
  contents: read
  id-token: write
```

It uses the protected environment and official crates.io authentication action:

```yaml
environment: crates-io

steps:
  - uses: actions/checkout@v4
    with:
      ref: ${{ github.ref }}
  - uses: rust-lang/crates-io-auth-action@v1
    id: crates-auth
  - name: Publish crates in dependency order
    env:
      CARGO_REGISTRY_TOKEN: ${{ steps.crates-auth.outputs.token }}
    run: bash scripts/publish_crates.sh --trusted "${GITHUB_REF_NAME#v}"
```

The OIDC token is short-lived and scoped by the crates.io trusted-publisher configuration. It is masked by GitHub Actions, never persisted, and unavailable to earlier build jobs.

### Trigger rules

- A signed version tag matching `v*` may build release artifacts.
- The publish job additionally requires the tag version to equal all public crate versions.
- A `workflow_dispatch` run defaults to dry-run and cannot publish.
- Authentication-only verification is available through `workflow_dispatch` behind the `crates-io` environment approval.
- Pull requests, forks, branch pushes, and GitHub Release edits cannot publish crates.

### Publication order

The trusted mode uses the same order and retry algorithm as bootstrap mode:

1. `rete-core`
2. wait for exact registry/index visibility
3. `rete-cli`
4. wait for exact registry/index visibility
5. `rete-wasm`
6. verify checksums and registry metadata

The same action publishes later RCs and `1.0.0`. No repository secret is introduced after bootstrap.

## Verification and Release Evidence

A successful crate-publication job proves:

- Tag, workspace version, crate versions, and exact internal dependencies agree.
- Source checkout is clean and matches the signed release tag.
- Packages build from their normalized registry manifests.
- RustSec, license/source policy, MSRV, rustdoc, compatibility fixtures, and stable API checks pass.
- The registry exposes all three exact versions.
- Downloaded registry archives match the locally generated package checksums.
- `cargo install rete-cli --version <release-version> --locked` succeeds in a clean container.
- A fresh crates.io consumer builds a small `rete-core` program and a wasm32 consumer checks `rete-wasm`.
- The non-secret publication receipt is attached to the corresponding GitHub Release.

The GitHub Release is not marked complete until crates.io and docs.rs verification passes for all three packages.

## Operational Responsibilities

The maintainer performs three manual operations:

1. Create and enter the one-time bootstrap token inside the devcontainer.
2. Revoke that token after `1.0.0-rc.1` is verified.
3. Configure the three crates.io trusted publishers and the protected GitHub environment.

All later publication is performed by the approved GitHub Actions job. Maintainers never need to create or rotate a long-lived registry secret for routine releases.

## Acceptance Criteria

- `rete-core`, `rete-cli`, and `rete-wasm` `1.0.0-rc.1` are manually published from the devcontainer in dependency order.
- The bootstrap token is revoked and absent from GitHub secrets, Cargo credentials, files, logs, and container volumes.
- Each crate trusts only `caviri/rete`, `release.yml`, and the `crates-io` environment.
- An authentication-only GitHub Actions run successfully exchanges OIDC for a short-lived crates.io token.
- A subsequent RC or `1.0.0` publishes all three crates through GitHub Actions without `CARGO_REGISTRY_TOKEN` stored as a secret.
- Partial retries verify existing checksums and never overwrite or silently skip mismatched versions.
- All registry packages have correct metadata, owners, features, license, README, repository links, and docs.rs builds.
- The GitHub Release contains a non-secret publication receipt matching the registry checksums.
