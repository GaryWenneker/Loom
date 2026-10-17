# Revision-pinned sparse exports

`loom sparse-export @CHANGESET DESTINATION [PREFIX ...]` materializes an
immutable native-tree selection into a new directory. With no prefix it exports
the complete revision. A prefix may name one file or a directory; overlapping
prefixes are deduplicated in canonical path order.

The command opens the revision through Loom's read-only snapshot capability. It
does not recover or lock the source worktree, move a branch, update a ref, or
read staged/unstaged source bytes. Every tree and blob is verified against its
canonical object ID. The destination must not exist, preventing accidental
replacement of another agent's workspace.

Each output file is created exclusively, written completely and synced. Only
after all selected files succeed does Loom create and sync
`.loom-sparse-complete`, recording the canonical root, file count and byte
count. A normal error or crash may leave a newly-created directory without that
marker. Such a directory is explicitly incomplete and must not be consumed;
remove it or choose a new destination before retrying.

The snapshot cache is bounded and files are materialized one at a time, providing
read/write backpressure instead of retaining the selected workspace in memory.
The current implementation deliberately favors bounded correctness over parallel
destination writes. Independent verified provider reads already support bounded
parallelism through grouped snapshot APIs, but this first export contract does
not yet pipeline listing, blob reads and filesystem writes.

## Reproduce the comparison

```sh
python3 benchmarks/sparse_export.py \
  --loom target/candidate/release/loom \
  --baseline-loom target/baseline/release/loom \
  --files 256 4096 --repeat 5 \
  --output sparse-export.json
```

The previous Loom binary receives the same immutable revision and prefixes. Its
compatibility client composes the public `files` and batch `query` commands and
syncs the same output bytes. Git uses `ls-tree` plus `checkout-index`. These are
equivalent user outcomes but not identical process/framing contracts: the new
native command needs one process, while the compatibility client and Git use
multiple commands. The report retains that difference instead of presenting it
as a pure internal algorithm comparison.

Every sample uses a fresh repository and destination, verifies all selected
paths and bytes, and records p50/p95/p99 wall/CPU, RSS, POSIX block counters and
logical/allocated output bytes. Setup and verification are outside timing.

## Limits

- Exports contain regular file bytes only. Executable bits, symlinks and other
  native-v2 metadata remain part of issue #13.
- The completion marker is an export receipt, not a Loom repository or mutable
  workspace registration. Branching, committing and ref CAS remain in the
  source repository.
- Directory entry durability across sudden power loss remains filesystem and
  platform dependent; no power-loss certification is claimed.
- A hostile process with write access to the new destination could race path
  creation. A future isolated-workspace authority must add anchored directory
  handles, ownership and per-workspace authorization.
- Remote sparse fetching, network-byte/cache-hit measurements, workspace leases,
  quotas and 100-agent soak/fault tests remain open under issue #26.

