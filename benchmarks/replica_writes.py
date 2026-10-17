#!/usr/bin/env python3
"""Measure checked all-ack publication to independent filesystem replicas."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import tempfile

from compare_publication import distribution, measured


def tree_bytes(root):
    logical = 0
    allocated = 0
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat()
            logical += stat.st_size
            allocated += getattr(stat, "st_blocks", 0) * 512
    return logical, allocated


def scenario(args, files):
    rows = {workers: [] for workers in args.workers}
    roots = {workers: [] for workers in args.workers}
    expected_root = None
    for repeat in range(args.repeat):
        order = args.workers[repeat % len(args.workers):] + args.workers[:repeat % len(args.workers)]
        for workers in order:
            with tempfile.TemporaryDirectory(prefix="loom-replica-writes-") as temporary:
                root = Path(temporary)
                workspace = root / "workspace"
                replicas = [root / f"replica-{index}" for index in range(args.replicas)]
                metrics, output = measured(
                    root,
                    [args.binary, str(workspace), str(workers), str(files)]
                    + [str(replica) for replica in replicas],
                )
                fields = output.decode().strip().split("\t")
                if len(fields) != 6:
                    raise RuntimeError(f"unexpected benchmark output: {output!r}")
                commit_ns, verification_ns, stored_objects, revision, canonical_root, checked = fields
                if len(canonical_root) != 64 or (expected_root is not None and canonical_root != expected_root):
                    raise RuntimeError("worker count changed the canonical fixture root")
                expected_root = canonical_root
                roots[workers].append(canonical_root)
                checked_counts = [int(value) for value in checked.split(",")]
                if len(checked_counts) != args.replicas or min(checked_counts) <= 0:
                    raise RuntimeError(f"incomplete replica verification: {checked_counts}")
                if int(stored_objects) < max(checked_counts):
                    raise RuntimeError("live verification omitted stored objects")
                if len(set(checked_counts)) != 1 or len(revision) != 64:
                    raise RuntimeError("replicas did not expose one identical checked revision")
                footprints = [tree_bytes(replica) for replica in replicas]
                rows[workers].append({
                    **metrics,
                    "commit_wall_ms": int(commit_ns) / 1_000_000,
                    "all_stored_objects_verification_ms": int(verification_ns) / 1_000_000,
                    "stored_objects_verified_per_replica": int(stored_objects),
                    "replica_logical_bytes": sum(value[0] for value in footprints),
                    "replica_allocated_bytes": sum(value[1] for value in footprints),
                    "objects_checked_per_replica": checked_counts[0],
                })
        print(f"Checked {files} files: repeat {repeat + 1}/{args.repeat}", flush=True)

    operations = {}
    for workers, samples in rows.items():
        operations[str(workers)] = {
            metric: distribution([sample[metric] for sample in samples])
            for metric in samples[0]
        }
    serial = operations[str(args.workers[0])]["commit_wall_ms"]["p50"]
    verified_serial = operations[str(args.workers[0])]["all_stored_objects_verification_ms"]["p50"]
    return {
        "files": files,
        "file_bytes": 256,
        "replicas": args.replicas,
        "canonical_roots_by_requested_workers": roots,
        "operations_by_requested_workers": operations,
        "worker_one_over_parallel_commit_p50": {
            str(workers): serial / operations[str(workers)]["commit_wall_ms"]["p50"]
            for workers in args.workers
        },
        "worker_one_over_parallel_verification_p50": {
            str(workers): verified_serial / operations[str(workers)]["all_stored_objects_verification_ms"]["p50"]
            for workers in args.workers
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=lambda value: str(Path(value).resolve()))
    parser.add_argument("--files", nargs="+", type=int, default=[100, 1000])
    parser.add_argument("--workers", nargs="+", type=int, default=[1, 2, 4])
    parser.add_argument("--replicas", type=int, default=4)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not hasattr(os, "wait4"):
        parser.error("per-process resource measurements require POSIX wait4")
    if min(args.files) < 1 or min(args.workers) < 0 or args.replicas < 2 or args.repeat < 1:
        parser.error("positive files/repeats, nonnegative workers and at least two replicas required")
    if len(set(args.workers)) != len(args.workers) or args.workers[0] != 1:
        parser.error("workers must be unique and start with the sequential control (1)")

    binary = Path(args.binary)
    report = {
        "schema_version": 2,
        "host": {
            "platform": platform.platform(),
            "cpus": os.cpu_count(),
            "cpu_model": next(
                (
                    line.split(":", 1)[1].strip()
                    for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith("model name")
                ),
                platform.processor(),
            )
            if Path("/proc/cpuinfo").exists()
            else platform.processor(),
        },
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "resource_helper_sha256": hashlib.sha256(Path(__file__).with_name("compare_publication.py").read_bytes()).hexdigest(),
        "metric_units": {
            "commit_wall_ms": "milliseconds; only Repository::commit",
            "all_stored_objects_verification_ms": "milliseconds; live per-object verification across every replica, enumeration excluded",
            "stored_objects_verified_per_replica": "objects; all stored objects including unreachable ones",
            "wall_ms": "milliseconds; process startup, fixture creation, commit and validation",
            "user_cpu_ms": "milliseconds; entire process",
            "system_cpu_ms": "milliseconds; entire process",
            "cpu_ms": "milliseconds; entire process",
            "max_rss_kib": "KiB; entire process",
            "rusage_inblock": "kernel counter units, not bytes",
            "rusage_oublock": "kernel counter units, not bytes",
            "replica_logical_bytes": "bytes summed across replicas",
            "replica_allocated_bytes": "st_blocks*512 bytes summed across replicas",
            "objects_checked_per_replica": "objects",
        },
        "conditions": [
            "fresh workspace and independent filesystem replicas for every sample",
            "identical deterministic 256-byte files in 64 leaf directories",
            "worker order rotates; explicit requests are capped by the Rust backend; zero selects automatic policy",
            "commit timing excludes fixture creation and post-commit verification",
            "live verification times every stored object immediately after commit; warm reads, same bounded worker request",
            "stored-object verification includes unreachable objects; it is not a graph traversal timing",
            "each acknowledged revision is reopened and fully fscked from every replica",
            "every replica's complete path list and all fixture payload bytes are verified after timing",
            "canonical native roots must match across all replicas, worker requests and repeats",
            "process CPU/RSS/I-O includes fixture creation and verification as well as commit",
            "OS caches are not flushed; no controlled cold-cache evidence",
            "local filesystem replicas only; network usage and remote latency are not measured",
            "no Git comparison: Git has no equivalent four-independent-store all-ack command",
            "this is one publisher; no multi-agent or crash/power-loss certification",
        ],
        "scenarios": [],
    }
    for files in args.files:
        report["scenarios"].append(scenario(args, files))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(
            json.dumps({
                "files": files,
                "worker_one_over_parallel_commit_p50":
                    report["scenarios"][-1]["worker_one_over_parallel_commit_p50"],
                "worker_one_over_parallel_verification_p50":
                    report["scenarios"][-1]["worker_one_over_parallel_verification_p50"],
            }),
            flush=True,
        )


if __name__ == "__main__":
    main()
