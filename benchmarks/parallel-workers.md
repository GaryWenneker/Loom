# Parallel object-worker benchmark

PR #85 bounds the worker fan-out to 32 threads. This is a resource-safety guard, not evidence of a speedup.

The opt-in worker-scaling microbenchmark has not yet been implemented. Do not run a nonexistent test command or report its results. Run future release-mode measurements on the same host, CPU governor and storage.

Compare 1, 2, 4, 8, 16, 32 and oversized worker requests for both small and large immutable-object batches. Capture raw timings, p50/p95/p99, CPU, peak RSS and bytes read/written. Separate CPU hashing from filesystem and remote object publication. Git comparisons must execute equivalent end-to-end workloads, not a different microbenchmark.

**Status:** No benchmark run or speedup measurement is asserted by this document. Loom remains unproven against the 100x performance target.
