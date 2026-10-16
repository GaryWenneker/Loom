# Loom scalability benchmark harness

This directory contains a **reproducible benchmark**, not Scion-level performance claims. It works with the Loom binary and Python 3.9+.

## Build and execute

```sh
cargo build --release -p loom-cli
python3 benchmarks/loom_bench.py --loom target/release/loom --files 100 1000 10000 --repeat 20 --output results.json
```

On Windows use `target/release/loom.exe`. A quick smoke run:

```sh
python3 benchmarks/loom_bench.py --loom target/release/loom --files 10 --repeat 1 --output /tmp/loom-smoke.json
```

For larger evaluations, use `--files 100000 1000000` and ensure you have disk space. The harness creates disposable repositories and removes them afterward.

## What it measures

- Time to `add` exactly one selected file amid other untracked files.
- Time to commit the frozen snapshot, with a growing sequence of first-parent commits.
- Time for `status` over the full untracked fixture, indexed direct branch file access (`cat`), checkout-free tracked path enumeration (`files`) and `fsck`.
- Raw CLI wall-clock samples plus p50/p95/p99 summaries and the binary SHA-256 hash.

The fixture generator is deterministic, spreads files across 256 directories, does not need Git or a server, and avoids silently committing its background data.

## Reading results

Compare each operation as fixture size increases. If `add` or a staged `commit` scales with unrelated files, investigate whole-worktree enumeration. If `status` scales linearly, add a trustworthy incremental index before claiming a Scion-like client.

Tail percentiles require ample repetitions. Distinguish cold/warm cache performance; report exact OS, filesystem, storage, CPU and RAM. Keep raw JSON output for review. This benchmark **does not** measure large tracked trees, remote RPCs, 100+ parallel writers, distributed cache hit rate, CPU peak, memory use or actual Scion outcomes.

## Automated evidence

The [Scalability evidence workflow](../.github/workflows/scalability.yml) runs weekly or manually from GitHub Actions. It builds the release binary, runs the 100/1,000/10,000-file suites with 20 commits each and uploads raw JSON samples as a versioned artifact (90-day retention).

Runner hardware may vary between runs. Always compare hardware metadata and binary hashes before attributing timing differences to Loom code. These observations are not release performance guarantees.

## Required follow-on benchmarks

The [Scion architecture assessment](../docs/scion-architecture-review.md) still requires:
1. 10k/100k/1m **tracked** path histories and ancestry/path index lookups.
2. Full/sparse on-demand checkout and blob streaming.
3. Parallel operation attempts at 1/10/100/500 agents.
4. Server persistence, cache hit ratios, memory/disk, CPU and p50/p95/p99.
5. Cross-platform crash and fault injections during those workloads.

Performance without lossless results and fault tolerance does **not** satisfy a release gate.

## Shared immutable provider stress

`python3 benchmarks/shared_object_stress.py --binary target/debug/loom --backend sqlite --agents 10 --commits 10` runs concurrent independent workspaces against one object database. Choose `--backend filesystem` for shared loose objects. It fails on any command error, missing acknowledged history, snapshot mismatch or `fsck` failure. Output is JSON; this is a local correctness workload, not distributed ref or failure-injection evidence.
