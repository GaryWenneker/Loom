# Changelog

Loom is an experimental project; neither its CLI nor its storage format has reached a stable contract.

## Unreleased

- Repository object-provider integration: external filesystem directories in the CLI, injectable durable object backends in the Rust API, verified publication and complete external history/blob/tree closure checks before ref publication.
- Explicit backend binding prevents silent fallback to a different object store. Volatile providers are rejected; full-store `fsck` reports unsupported enumeration explicitly.

- Continued engineering review, onboarding documentation and platform architecture work.
- Open PRs and planned features must not be treated as shipped.

## 0.1.0: experimental native alpha

- Immutable BLAKE3-addressed blobs and changesets.
- SQLite refs and initial commit/history/branch CLI.
- Opt-in immutable staging, scoped diffs and combined change review.
- Conservative three-way line merge.
- Local cross-process worktree lock, initial recovery intent journal, and basic integrity checks.

The listed features are experimental and do not imply production-ready recovery or remote compatibility.