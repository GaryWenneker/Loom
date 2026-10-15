# Loom

**Loom** is an experimental, independent native version control system implemented in Rust. It is not an extension of Git, or of any other product.

> **Early development:** Never use Loom as the only copy of valuable source code. Native format v1 is experimental and may change.

## Architecture

- Content-addressed immutable objects using BLAKE3-256 and a native canonical binary envelope.
- Embedded object store and SQLite transactionally managed references.
- Native changeset DAG, with changes relative to the first parent.
- Dedicated CLI, with distributed services and Git bridging tracked separately.
- Works independently of Git; compatibility belongs to an isolated adapter.

See [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), and [format spec](docs/format-v1.md).

## Build (after implementation is complete)

```bash
cargo build --workspace
cargo test --workspace
```

Planned command examples: `loom init`, `loom status`, `loom commit -m "message"`, `loom log`, `loom fsck`.

## Status and trust

This repository is being built incrementally. Check GitHub Actions and the roadmap for which capabilities are **implemented and tested**, not merely designed. Merge, sync, distributed replication and production hardening are not yet claimed.

## License

MIT.
