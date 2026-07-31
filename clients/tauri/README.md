# Rete File Explorer — desktop

The [rete file explorer](../../experiments/rete-file-explorer) as an installable
app. Same page, different engine underneath.

## Why a native build at all

In the browser the UI drives a wasm `RemoteGraph` in a Web Worker. Here it drives
`rete-core` directly, which buys things the browser cannot give:

- **No wasm, no 4 GB heap ceiling.** A 17 GB local `.rete` is a positional read
  away instead of impossible.
- **Real threads** — `rete-core`'s `parallel` feature is on.
- **`mmap`-shaped local reads.** Opening a local file goes through the same lazy
  `RangeReader` path the HTTP client uses, so a huge graph faults in rather than
  being read whole.

One file differs between the two builds: `app.js` picks its transport at runtime.

```js
const w = isTauri() ? makeTauriWorkerShim() : new Worker("./js/fs-worker.js");
```

`tauri-bridge.js` makes the Rust commands speak the worker's message protocol
(`open` / `query` / `stats`, echoing `reqId`, attaching `stats` to every reply),
so `rete-fs.js` — the entire projection: five views, listings, extract — is
byte-identical in both. That was the claim the experiment's split was making;
this is the proof.

## Build it locally

Needs the [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/)
for your platform, plus Rust.

```sh
bash clients/tauri/scripts/sync-frontend.sh   # stage the UI into dist/
cd clients/tauri/src-tauri
cargo tauri icon icons/source.png             # once — generates .icns/.ico/pngs
cargo tauri dev                               # or: cargo tauri build
```

`dist/` and the generated `icons/*` are build outputs and are not committed.

## Releases

`.github/workflows/desktop-release.yml` builds on macOS runners and attaches the
`.dmg`s to a GitHub Release. Trigger it from the Actions tab (**Run workflow**,
on demand) or by pushing an `explorer-v*` tag. It produces two artifacts:

| Runner | Target | For |
|---|---|---|
| `macos-14` | `aarch64-apple-darwin` | Apple Silicon (M1 and later) |
| `macos-13` | `x86_64-apple-darwin` | Intel Macs |

### Why not Docker

There is no Docker path to a macOS app, and this is not a tooling gap. The macOS
SDK is licensed for use on Apple hardware, so no legitimate image carries it, and
`codesign`/`notarytool` are macOS-only binaries. `osxcross` can cross-compile if
*you* supply the SDK, but the result still cannot be notarized — and an
un-notarized `.dmg` is refused by Gatekeeper, which defeats the point of shipping
something people install.

Windows is different: `cargo-xwin` genuinely cross-compiles `x86_64-pc-windows-msvc`
from Linux. The bundlers are the catch — NSIS and WiX both want Wine in the image,
and WebView2 bootstrapping is fiddliest exactly there. A `windows-latest` runner
costs nothing and avoids debugging Wine instead of the app. Docker still earns its
place for a Linux `.AppImage`, where a pinned glibc/webkit2gtk is the whole point.

## These builds are unsigned

macOS will refuse to open them on first launch — "cannot be opened because the
developer cannot be verified", or, if the `.dmg` was downloaded through a
browser, "is damaged and can't be opened". Neither is true; both are the
quarantine attribute doing its job on an app with no Developer ID signature.

One-time bypass, after dragging the app to Applications:

```sh
xattr -dr com.apple.quarantine "/Applications/Rete File Explorer.app"
```

Or right-click the app → **Open** → **Open** in the dialog.

Making this go away is a credentials problem, not a code one: an Apple Developer
Program membership ($99/yr) to notarize. When those exist, add
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` as repository secrets — Tauri
picks them up and the workflow's shape does not change.

## What the desktop build does not do yet

- **`prefix` / `text` search** are not wired to native commands. The explorer
  does not call them, so nothing is missing in the UI; the bridge fails loudly
  rather than returning an empty result if something ever does.
- **The Turtle tab shows fully expanded triples.** The prefix-folding writer
  lives in the wasm crate; duplicating it here would be a second implementation
  to keep honest for a cosmetic gain. N-Triples is a valid subset of Turtle.
- **No file-type association yet** — double-clicking a `.rete` does not open the
  app. That is a `bundle.fileAssociations` entry plus an open-event handler.
