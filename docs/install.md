# Install rete

Choose how you want to use rete, then copy the commands for your system.
The command-line tool builds and queries graph files; the language packages
add the engine to your application. To try a graph without installing anything,
open the [Playground](playground-guide.md).

<!-- install-selector -->

## Choose the right package

Docker is a convenient way to try the CLI without a Rust toolchain. Cargo
builds the native CLI on your machine. If you prefer a ready-made executable,
the [0.7.1 release](https://github.com/caviri/rete/releases/tag/v0.7.1) has
Linux, macOS and Windows archives for x86-64 and ARM64, plus checksums.
Choose the archive matching your operating system and processor.

Python, JavaScript and Rust packages are libraries. Installing one does not
install the native `rete` command. The selector uses a virtual environment for
Python and adds JavaScript/Rust dependencies to your project. For other
integrations, see [R](r.md), [Java and other clients](clients.md) or
[Tools and integrations](integrations.md).

## Match the reader to your files

These commands pin the published **0.7.1** packages, verified against GitHub,
[PyPI](https://pypi.org/project/rete-graph/0.7.1/),
[npm](https://www.npmjs.com/package/rete-graph/v/0.7.1) and
[crates.io](https://crates.io/crates/rete-cli/0.7.1) on 2026-11-12.
They write format `0x08` and read `0x05` to `0x08`. Readers up to 0.6.x
cannot open `0x08` files. Check [Compatibility](compatibility.md) before
sharing files with older readers.

For the current source toolchain, follow [Development](contributing.md).
For a complete first task with the published CLI, continue to the
[Quickstart](getting-started.md).
