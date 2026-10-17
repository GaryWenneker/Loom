# Bounded parallel text merges

Loom can prepare independent three-way text merges concurrently without changing
its canonical object format or branch publication protocol.

## Data-flow and correctness contract

The repository first walks the sorted union of base, local and remote paths.
Paths that choose an unchanged side are resolved immediately. Paths requiring a
real three-way text merge become immutable tasks containing only the path and
three blob IDs.

One verified object read session processes those independent tasks with bounded
workers. Automatic mode keeps batches below 128 real text merges sequential,
avoiding the scheduling overhead observed in the checked 64-file workload. From
128 tasks onward it uses at most two workers, the largest setting with a measured
benefit in the checked baseline. Explicit non-zero requests remain available for
scaling experiments. Both modes are capped by the task count, provider hint,
eight workers and available CPUs. Every scoped worker is joined on success,
error or panic; no background merge I/O survives the call.

Results are returned in original path order. Loom computes canonical blob
envelopes and IDs, checks file/directory path collisions, and rejects the entire
merge if any text path conflicts. Importantly, no successful sibling merge blob
is published before all paths are known to be conflict-free. Only then are the
ordered canonical envelopes sent through the existing bounded object publisher.

The native tree is constructed from this deterministic map. Worktree
materialization still occurs through the durable transition journal, and the
branch remains unchanged until the existing compare-and-swap ref transaction.
Parallel preparation therefore cannot expose a partial snapshot or bypass CAS.

## API and limits

`Repository::merge_branch` uses adaptive provider-safe concurrency.
`Repository::merge_branch_with_workers` allows embedders and the benchmark to
request a specific bound; zero selects adaptive mode and requesting more never
exceeds the safety caps.

This implementation parallelizes independent text-merge preparation. It does not
parallelize merge-base recursion, worktree materialization, ref transactions,
rename detection, structured conflict resolution or binary merging.

## Reproduce the checked comparison

```sh
cargo build --release --locked -p loom-core --example parallel_merge
python3 benchmarks/parallel_merge.py \
  --binary target/release/examples/parallel_merge \
  --files 64 512 --workers 1 2 0 --repeat 7 \
  --output parallel-merge.json
```

Each sample creates fresh Loom or Git histories with identical text files. One
branch changes the first line and the other changes the last line of every file.
Fixture creation is outside timing. The measured operation is Loom's merge call
or Git's `merge --no-ff` command. Afterwards the harness verifies every byte,
clean status and full fsck; Loom also requires identical native roots for the
explicit sequential, explicit parallel and adaptive modes.

The report retains p50/p95/p99 wall/CPU, peak RSS, POSIX block-I/O counters and
logical/allocated metadata bytes. Engine order rotates. OS caches are not
flushed, and network traffic, remote caches, concurrent agents, merge conflicts,
crashes and power-loss behavior are not measured. Results describe these local
independent-text workloads only and are not a universal speed claim.
