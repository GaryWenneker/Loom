# Contributing to Loom

Loom is an experimental native version control system. Correctness, portability, and preventing data loss take priority over adding commands.

## Workflow

1. Search existing issues and PRs before beginning work. Open a narrowly scoped issue for major changes.
2. Explain expected behavior, format or API changes, and potential data-loss consequences in the PR.
3. Add negative, corrupt-input, concurrent-operation, and crash/recovery tests where the feature touches persistent state.
4. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace --all-targets`.
5. Keep documentation, the support matrix, and roadmap aligned with merged code.
6. Verify required checks on the latest PR commit, including Linux/macOS/Windows and the dependency audit.

## Engineering standards

Objects are immutable; changing content requires a new ID. Refs must use conditional updates and cannot reference missing data. Workspace operations must preserve unrelated staged, tracked, ignored, and untracked user files. Avoid panics on external input, unbounded allocations, and unsafe path traversal.

Keep the native object engine independent of optional hosting and Git adapters. Explain measured performance improvements with reproducible workloads, not intuition.

See [architecture](docs/architecture.md), [code review findings](docs/code-review.md), and [release gates](docs/release-gates.md).

## Security and license

Report vulnerabilities privately using [SECURITY.md](SECURITY.md). Contributions are made under [MIT](LICENSE).
