# Verified replica closure — 2026-10-17

A replicated backend now establishes object closure from live canonical bytes on every replica. A stale existence flag or a healthy first copy cannot acknowledge a corrupt secondary copy. Ref CAS, immutable object IDs and staging semantics are unchanged.

Measured [source 645aad45](https://github.com/GaryWenneker/Loom/commit/645aad4579aafac64a977d5910e39c320b59c547), tree `a4d9dbf96bd461696169fad7d6d545ad9f98f3c5`, based on main `4260d72f1573394813ddab5c46b1c3f309e0e33e`. Evidence commits do not change measured Rust or the harness.

## Method

Rust 1.99.1 release build on the current Intel Xeon Platinum 8573C / Linux 6.18.55 host (nine reported CPUs). Four independent filesystem stores receive fresh deterministic 256-byte fixtures for every sample. Eight repeats rotate requested replica workers 1, 0, 2 and 4 through two complete order cycles. No local tests or builds run concurrently with this reported run. Host scheduling and OS caches remain uncontrolled.

Request 0 uses up to four publication workers and one verification worker. Explicit requests control both replica phases, capped at eight and replica count. These requests do not make the whole engine single-threaded: the worktree scanner and other engine phases retain their own policies.

The first timer covers Repository::commit only, including publication and required closure checks. A separate timer subsequently verifies every stored object across all replicas, including unreachable objects, and excludes object enumeration. This second timer uses stores warmed by the commit; it is not a cold-storage, remote-network or graph-traversal result.

After both timers, every replica is reopened, its entire path list and every fixture payload byte are checked, and full fsck runs. Native roots must agree across all copies, requests and repeats. Revision IDs may differ because changeset timestamps are real.

## Latency distributions

| Files | Replica request | Commit p50 ms | p95 | p99 | All-stored-object verification p50 ms | p95 | p99 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 | 1 | 846.25 | 1163.07 | 1245.27 | 4.25 | 12.81 | 14.27 |
| 32 | 0 | 461.39 | 517.77 | 529.35 | 4.90 | 6.87 | 7.03 |
| 32 | 2 | 691.68 | 1575.06 | 1840.28 | 28.41 | 35.47 | 36.68 |
| 32 | 4 | 577.52 | 880.56 | 988.52 | 47.29 | 75.44 | 83.26 |
| 128 | 1 | 2972.67 | 4855.25 | 5193.86 | 16.61 | 25.90 | 29.22 |
| 128 | 0 | 1899.15 | 2771.07 | 2998.95 | 14.37 | 22.08 | 22.23 |
| 128 | 2 | 2289.54 | 3650.87 | 3754.67 | 96.35 | 192.03 | 195.07 |
| 128 | 4 | 2287.20 | 3821.01 | 4047.50 | 175.40 | 353.55 | 364.90 |

## Interpretation

- 32 files: automatic commit 461.39 ms versus 846.25 ms with one replica worker (1.83×). Four-worker warm verification is 11.13× slower than one worker; the regression is retained.
- 128 files: automatic commit 1899.15 ms versus 2972.67 ms with one replica worker (1.57×). Four-worker warm verification is 10.56× slower than one worker; the regression is retained.

These are same-binary replica scheduling controls with the same verification guarantee. They are not an old/new correctness comparison: the pre-fix existence flags did less verification. Extra workers are not assumed beneficial for short local reads. The default therefore keeps verification serial while overlapping durable publication. No remote-latency benefit is established. Eight samples provide descriptive quantiles, not production tail-latency SLOs.

Git has no directly equivalent single command that acknowledges canonical closure on four independent durable object stores. No Git speed factor is fabricated. This evidence does not meet the 100× Git goal.

## Resources and limits

Raw JSON retains every sample and p50/p95/p99 for whole-process CPU/wall time, raw wait4 maximum RSS, POSIX block-I/O counters and summed logical/allocated replica bytes. Process resources include fixture creation, both timed phases and all post-commit checks; they cannot be attributed solely to commit or verification. RSS may include the launcher floor, kernel block counters are not portable bytes, and the bounded per-read payload / worker count is not a total RSS cap.

Network bytes, controlled cold caches, cloud providers, concurrent agents, crash/power-loss durability and isolated engine-memory reductions are unmeasured. Per-call worker caps are not a global budget for unrelated concurrent calls. Ordinary get reads the first verified available copy; there is no read repair or quorum downgrade. The immutable-provider contract excludes later same-key mutation after verification.

## Correctness

237 local workspace/all-target tests pass, with formatting and strict Clippy. Six new regressions cover corrupt secondary data, lying existence flags, hash-matching malformed envelopes, actual bounded read overlap, joined errors/panics, and ref/staging preservation plus repair/retry and independent reopen of acknowledged history. The benchmark target is additionally built and exercised with full payload/root checks.

The complete product remains an experimental local alpha. Native v2 migration, secure remote authority/protocol, isolated sparse agent workspaces and #26 scale/reliability gates remain open. Neither a green build nor this storage fix establishes production readiness or overall 100% completion.

Binary SHA-256: `9c26a1bdb68c484feec895650e5d54661eec5985e3b31e167bca9c1de987d1f1`. Harness: `536408555255bb898efa67ecb11b0fafc25e15cba701df6b9dc6f1fa65a74b73`. Compiler, source/tree and raw-data hashes are in the [provenance](../benchmarks/results/replica-verification-20261017-provenance.json). [Raw results](../benchmarks/results/replica-verification-20261017.json), [contract and reproduction](bounded-replica-publication.md).
