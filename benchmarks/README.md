# Loom scalability benchmark harness

This directory contains a **reproducible benchmark**, not Scion-level performance claims. It works with the Loom binary and Python 3.9+.

## Portable migration preflight

```sh
python3 benchmarks/portable_migration_plan.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --files 100 1000 \
  --workers 1 2 4 0 --repeat 5 --output portable-migration-plan.json
```

Each timed operation plans the complete immutable v1 snapshot from a fresh
repository. Candidate and baseline receive byte-identical payload directories
and identical explicit worker arguments; unsupported baselines have no version
factor. Git `fsck` is retained only as a verified-closure diagnostic. The
report includes p50/p95/p99 wall/CPU, raw wait4 RSS and block counters, exact
argv, logical/allocated repository footprints and canonical roots. It does not
flush caches or measure network, applied migration, rollback, crashes or agents.

## Atomic selected-set workflows

```sh
python3 benchmarks/pathsets.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --files 1000 10000 100000 \
  --directories 256 --changed-paths 64 --repeat 6 --scan-workers 8 \
  --output pathsets.json
```

This checks complete add+commit commands, both startups included, for the
requested number of modified files and independent top-level subtrees. Initial repository creation is
outside timing. The first unchanged add records cold index bootstrap separately;
incremental samples then use current native path indexes. Existing repositories
are reused with identical history depth, and engine order rotates. For baselines with recursive add, pass `--baseline-directories` and use the
same `--scan-workers N --baseline-scan-workers N` requests. Both engines then
receive identical directory scopes and worker policies. Omitting the baseline
directory flag retains a legacy expanded-file control for older binaries;
argument counts differ and it is not a same-input directory comparison. Git receives the same
file/directory selections as new Loom.
Run the harness with `--scan-workers 1` and `--scan-workers 8` (or omit the
option for up to four workers) to compare identical sequential and
bounded-parallel selected-set data paths. The worker request applies to candidate
Loom reads/hashes and commit-time independent subtree construction; the phases
do not overlap and therefore share one bounded request. It does not change the
separately bounded immutable-object publisher.

`Selected-set worker evidence` runs for relevant pull requests and can also be
dispatched manually for the larger 1k/10k/100k experiments. It builds the selected candidate and resolved baseline
commit in separate target directories, retains their commit/tree/compiler/binary
identities and uploads the checked raw results. Ordinary Rust CI uses small
one/eight-worker correctness runs. The large workflow runs the two requests in
sequence on one runner, with fresh repositories and uncontrolled caches; compare
each candidate with its paired baseline/Git samples and do not infer a worker
speedup from separate runs alone. The measurements retain p50/p95/p99 wall, CPU, raw wait4 RSS and kernel I/O
counters. They do not isolate cold cache, allocated disk blocks, network use or
per-process peak RSS. No speedup is claimed by exposing the worker control.

Each sample checks every listed path, selected payload bytes/framing and old/new
native root equality. Full final payloads and full history/closure fsck are checked
after all samples. This is explicit user/agent selection, not an automatic change
journal or a speed claim for legacy full scans. POSIX resource measurements share
the wait4 launcher-floor and kernel-I/O-unit limitations documented below.

[Measured selected-set workflows](../docs/performance-pathsets-20261017.md)
include all favorable/unfavorable controls and cold-index bootstrap observations.

## Durable publication workflows

```sh
python3 benchmarks/compare_publication.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --files 1000 10000 --repeat 6 \
  --layout sharded --directories 256 --output publication-sharded.json
```

This POSIX harness measures complete initial commit workflows in fresh identical
fixtures, then a commit with eight modified files. It also checks changed status
before that commit and clean status after full verification. Loom uses commit-all; Git uses
both `add` and `commit` inside the measured workflow. Startup is included.
Fixture creation, full payload/listing verification and fsck occur outside timing.
All native roots must match between old/new Loom. Engine order rotates; six
repeats cover two full three-engine cycles. Try `--layout nested --directories
1024 --files 10000` and `--files 100000` separately.

`--scan-workers N` applies only to the new Loom binary; omit it for automatic
selection. Compare 1/2/4/8/16/32/1024 separately; requests above eight are capped,
and small windows remain serial. These complete workflows also include the
unchanged publication-worker policy, so this is scan scheduling evidence, not
an isolated worker microbenchmark. All file bytes, statuses and native roots
are checked regardless of the requested limit.

`wait4` records per-process CPU and maximum RSS; Git's two commands have summed
wall/CPU/I-O and maximum RSS. Raw kernel block-I/O counters are retained in their
OS units, rather than presented as portable byte counts. Metadata size is logical
file length, not disk allocation. The raw `wait4` RSS high-water can include a
fork/pre-exec launcher floor; it is not an isolated engine-RAM measurement.
Samples include p50/p95/p99; few samples are insufficient for production tail
guarantees. This filesystem-provider benchmark
does not flush OS caches or measure network, power loss or concurrent agents.

## Full-content scan worker scaling

```sh
python3 benchmarks/scan_workers.py --loom /absolute/new-loom \
  --files 1000 10000 100000 --workers 1 2 4 8 16 32 1024 --repeat 7 \
  --directories 256 --output scan-workers.json
```

This measures complete status commands over clean, eight-file-modified and
all-files-modified worktrees. Each size uses one fresh committed fixture. Exact
sorted status output, working bytes and unchanged HEAD are checked, with fsck
after each phase. Worker order rotates; seven repeats give one complete cycle.
This is Loom sequential/parallel scan evidence; use the workflow harness above
for Git comparisons. It shares the POSIX resource-accounting limitations above.

[Measured scan results](../docs/performance-worktree-scans-20261017.md) retain all
default-worker, larger-payload, nested-directory and worker-scaling controls.

## Repeated immutable working sets

```sh
python3 benchmarks/compare_git.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --baseline-multi --batch --multi \
  --layout nested --directories 1024 --files 10000 --rounds 100 --repeat 8 \
  --requests 1000 --output working-set-nested.json
```

Schema 4's `--rounds` repeats and rotates one working set within each persistent
process. Both Loom versions and packed Git receive the identical expanded path
sequence and return every logical answer. No per-file Git processes, omitted
bytes, read-pack preparation or off-timer query warmup are used. The initial
group, process startup, parsing/framing and all responses remain in the elapsed
time. With 100 rounds and 1,000 paths, that is 100,000 logical requests; each
round uses the same working set. Engine caches warm naturally after the first
group. This does not represent 100,000 distinct file paths or cold storage.

Timings describe the complete sequence, not per-response p50/p95/p99. Fresh-session
first-group controls (`git_first_group`, `loom_first_group` and optional
`baseline_first_group`) run in the same fixture and are reported separately.
Single CLI/status controls remain one operation each. Inputs are capped at 100
rounds and 100,000 total requests; the existing per-line query limits still
apply. Repeat with `--layout sharded --directories 256 --files 10000 100000`.
Eight modes with the previous binary/no pack use eight rotated runs for a full
order cycle. With different flags, choose repetitions appropriately. Avoid a
general speed claim based on a cache-hot operation.

Use `--cache-bytes 0` to disable both Loom query caches, or specify a budget
through 256 MiB. The same value is passed to old/new Loom and retained in
`cache_budget_bytes`; Git keeps its normal process cache. This flag affects
query modes, including fresh-session controls, rather than the independent
cat/files/status CLI controls. For a first-read comparison without cache reuse:

```sh
python3 benchmarks/compare_git.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --baseline-multi --batch --multi \
  --layout nested --directories 1024 --files 10000 --cache-bytes 0 \
  --rounds 1 --repeat 5 --requests 1000 --output directory-zero-cache.json
```

Five modes with these flags use five rotated repeats for one complete order
cycle. OS caches remain warm; zero Loom cache does not imply cold storage.

For a higher-sample native confirmation without repeatedly timing CLI status:

```sh
python3 benchmarks/compare_native_group.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --files 100000 --directories 256 \
  --repeat 50 --requests 1000 --output native-pairs.json
```

This creates one fixture with 11 commits, then alternates engine order for
paired seeds in fresh zero-cache query processes. Every returned byte/slot is
checked. Raw samples, ratios, host metadata and binary/harness hashes are saved.
It has no Git comparison and should supplement, rather than replace, the
equivalent Git controls and their unfavorable results.

## Explicit read-pack comparison

```sh
python3 benchmarks/compare_git.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --baseline-multi --batch --multi --read-pack \
  --layout sharded --directories 256 --files 1000 10000 100000 \
  --repeat 7 --requests 1000 --output read-pack-sharded.json
```

The previous binary initializes the same fixture; `--baseline-multi` requires a
baseline with `cat_many` support (main #79 or later). Compare identical native
groups in `baseline_multi`, `loom_multi` and `loom_pack_multi`, and the existing
line interface separately. The pack is built once; build milliseconds and file
bytes are reported outside query timing. All live objects remain. Each timed
query starts a fresh process and loads/checks the index again. Git packing and
commit-graph preparation are likewise outside its query timing. Schema 3 retains
unpacked results and all single-command/status controls, including regressions.
`unpacked_over_packed_p50` compares the same new binary with/without the explicit
pack. Repeat with `--layout nested --directories 1024 --files 10000` to check
deeper hierarchies. OS caches are warm; RSS/CPU/network/concurrency remain unmeasured.

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

The [Scalability evidence workflow](../.github/workflows/scalability.yml) runs weekly or manually from GitHub Actions. It also publishes checked comparisons against packed Git for wide trees through 100,000 files and a sharded 10,000-file control. It builds the release binary, runs the 100/1,000/10,000-file suites with 20 commits each and uploads raw JSON samples as a versioned artifact (90-day retention).

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

## Equivalent Git comparison

```sh
cargo build --release --locked -p loom-cli
python3 benchmarks/compare_git.py --loom target/release/loom --batch \
  --files 1000 10000 100000 --repeat 5 --requests 1000 \
  --output compare-git-results.json
```

The disposable fixture gives both engines identical tracked binary bytes and
pins immutable revisions. Git uses packed objects and a changed-path commit
graph by default; `--git-layout loose` exercises loose objects. Batch compares
one Loom query process against one `git cat-file --batch` process, with 1,000
path requests (10% misses), checking every returned byte and frame. Single-file
reads, scoped listings and clean status are measured separately. Inherited Git
configuration/provider variables are removed, and engine order alternates.

The intentionally **wide directory** fixture exposes repeated tree decoding;
it does not represent every repository layout. Use `--layout sharded` as a
control with files spread over 256 directories. Samples include startup with
warm OS caches. Reports include binary/harness hashes, hardware metadata, raw
samples, percentiles, logical repository bytes and per-operation 100× target
flags. Five samples give only a rough tail estimate. Peak RSS, CPU time, cold
storage, network access and concurrent clients are not measured. Never infer
an overall speed guarantee from a batch result. In particular, clean status
still scans file contents and remains an explicit bottleneck.

[Measured 2026-10-16 results](../docs/performance-20261016.md) include raw wide and
sharded samples and explicitly retain operations that miss the 100× goal.


## Multi-directory before/after comparison

```sh
python3 benchmarks/compare_git.py --loom /absolute/new-loom \
  --baseline-loom /absolute/previous-loom --batch --multi \
  --layout sharded --directories 256 --files 1000 10000 100000 \
  --repeat 7 --requests 1000 --output multidirectory.json
```

Both binaries must be built in release mode with the same compiler; keep immutable
copies. The baseline initializes the shared fixture. Each repeat uses identical
paths/bytes for previous Loom, new line-oriented Loom, new native `cat_many` and
Git's persistent batch. Engine order rotates. Native grouping changes request
framing, so it is reported separately from the unchanged line interface.
`baseline_over_loom_p50` reports improvement over previous Loom, not Git.

Use `--layout nested --directories 1024 --files 10000` to exercise two directory
levels beneath `files/`. Single-command and status results remain in the report.
`--multi` accepts at most 4,096 logical paths and the encoded request must fit the
protocol's 64 KiB line cap. The existing 100× target flags apply per operation;
all unfavorable results are retained. No performance gate relies on noisy CI timing.

[Measured multi-directory results](../docs/performance-multidir-20261016.md) compare
the previous binary, unchanged line API, native grouping, and packed Git.
