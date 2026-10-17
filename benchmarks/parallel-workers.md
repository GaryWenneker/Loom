# Parallel object-worker benchmark

PR #85 bounds the worker fan-out to 32 threads. This is a resource-safety guard, not evidence of a speedup.

Run a release-mode benchmark on the same host, CPU governor and storage:

```sh
cargo test --release -p loom-core --lib worker_scaling_benchmark -- --ignored --nocapture
```

Compare 1, 2, 4, 8, 16, 32 and oversized worker requests for both small and large immutable-object batches. Capture raw timings, p50/p95/p99, CPU, peak RSS and bytes read/written. Separate CPU hashing from filesystem and remote object publication. Git comparisons must execute equivalent end-to-end workloads, not a different microbenchmark.

**Status:** No benchmark run or speedup measurement is asserted by this document. Loom remains unproven against the 100x performance target.
