# Getting started

**Loom 0.1 is an experimental alpha. Use a disposable repository and keep a separate backup.**

## Build

Install Rust 1.85 or newer, clone the Loom repository, then run:

```sh
cargo build --release -p loom-cli
cargo test --workspace --all-targets
```

The CLI is `target/release/loom` on macOS/Linux or `target\\release\\loom.exe` on Windows. You can also install with `cargo install --path crates/loom-cli`.

## Create a repository

```sh
mkdir loom-demo
cd loom-demo
loom init
echo "Hello" > example.txt
loom status
loom add example.txt
loom review
loom commit -m "Initial snapshot" --author "Developer"
loom log
```

The first `loom add` enables explicit staging for future commits. Staging captures the file bytes at that instant; later edits remain separate.

## Branches

```sh
loom branch create feature
loom switch feature
echo "Feature" > feature.txt
loom add feature.txt
loom commit -m "Add feature"
loom switch main
loom diff main feature
loom cat feature feature.txt
loom fsck
```

A dirty or untracked worktree can block branch switching. A basic conservative merge is available, but full interactive conflict resolution is not.

## Known limitations

No remote sync, Git bridge, tags, undo/rebase, signed metadata, symlinks, executable-bit preservation or large-file chunking. Maximum v1 object payload: 64 MiB. The recovery journal covers selected process-crash cases only, not guaranteed power-loss durability.

See the [CLI reference](cli-reference.md), [architecture](architecture.md), [roadmap](roadmap.md) and [release gates](release-gates.md).
