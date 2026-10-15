#!/usr/bin/env python3
"""Reproducible Loom CLI microbenchmarks, not a claim of Scion parity.

Uses only Python's standard library. To measure native scalability, run
across sizes on the same host and inspect *raw* per-operation samples.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone


def percentile(values: list[float], pct: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    pos = (len(ordered) - 1) * pct
    lo = int(pos)
    hi = min(lo + 1, len(ordered) - 1)
    return ordered[lo] * (1.0 - (pos - lo)) + ordered[hi] * (pos - lo)


def run(executable: Path, root: Path, *args: str) -> float:
    begin = time.perf_counter_ns()
    result = subprocess.run(
        [str(executable), *args], cwd=root, stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE, check=False, timeout=180,
    )
    elapsed = (time.perf_counter_ns() - begin) / 1_000_000
    if result.returncode != 0:
        raise RuntimeError(
            f"loom {' '.join(args)} failed: " +
            result.stderr.decode(errors="replace")[:2000]
        )
    return elapsed


def fixture(root: Path, count: int) -> None:
    # Spread paths across 256 directories for reproducible fanout.
    for index in range(count):
        folder = root / "data" / f"{index % 256:02x}"
        folder.mkdir(parents=True, exist_ok=True)
        (folder / f"{index:08d}.txt").write_bytes(
            f"fixture:{index:08d}:constant-content\n".encode("ascii")
        )


def sample(executable: Path, count: int, repeat: int, skip_status: bool) -> dict:
    results: dict[str, list[float]] = {
        "add_ms": [], "commit_ms": [], "status_ms": [],
        "cat_ms": [], "files_ms": [], "fsck_ms": [],
    }
    with tempfile.TemporaryDirectory(prefix="loom-bench-") as temp:
        root = Path(temp)
        init_ms = run(executable, root, "init")
        start = time.perf_counter()
        fixture(root, count)
        fixture_seconds = time.perf_counter() - start
        # Enable staging before any commit. Untracked fixture data must not be
        # silently committed: it emulates large real working directories.
        file = root / "focus.txt"
        file.write_bytes(b"baseline")
        for iteration in range(repeat):
            file.write_bytes(f"iteration-{iteration:06d}".encode())
            results["add_ms"].append(run(executable, root, "add", "focus.txt"))
            results["commit_ms"].append(
                run(executable, root, "commit", "-m", f"iteration {iteration}")
            )
            results["cat_ms"].append(
                run(executable, root, "cat", "main", "focus.txt")
            )
            results["files_ms"].append(
                run(executable, root, "files", "main", "focus.txt")
            )
            if not skip_status:
                results["status_ms"].append(run(executable, root, "status"))
        results["fsck_ms"].append(run(executable, root, "fsck"))
        # Assert that the benchmark did not consume unrelated fixture files.
        expected = root / "data" / "00" / "00000000.txt"
        if count and not expected.exists():
            raise RuntimeError("benchmark modified the worktree unexpectedly")
    return {
        "untracked_fixture_files": count,
        "commits": repeat,
        "init_ms": round(init_ms, 3),
        "fixture_creation_seconds": round(fixture_seconds, 3),
        "operations": {
            name: {
                "samples_ms": [round(x, 3) for x in values],
                "p50_ms": round(percentile(values, 0.50), 3),
                "p95_ms": round(percentile(values, 0.95), 3),
                "p99_ms": round(percentile(values, 0.99), 3),
            }
            for name, values in results.items() if values
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True, help="Path to compiled loom binary")
    parser.add_argument("--files", type=int, nargs="+", default=[100, 1000, 10000])
    parser.add_argument("--repeat", type=int, default=10)
    parser.add_argument("--skip-status", action="store_true")
    parser.add_argument("--output", default="loom-benchmark-results.json")
    args = parser.parse_args()
    exe = Path(args.loom).resolve()
    if not exe.is_file():
        parser.error(f"binary does not exist: {exe}")
    if args.repeat < 1 or any(x < 0 or x > 1_000_000 for x in args.files):
        parser.error("repeat must be positive; file counts must be in [0, 1000000]")
    report = {
        "schema_version": 1,
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
        },
        "loom_executable_sha256": hashlib.sha256(exe.read_bytes()).hexdigest(),
        "notes": [
            "Untracked fixtures only. This measures targeted selected-file add and commit, "
            "plus indexed branch file lookups, full status and native fsck. "
            "It does not measure giant tracked histories.",
            "Timings include CLI process startup. Low sample counts make tail percentiles unreliable.",
            "No RAM/CPU or Scion measurements are claimed by this harness.",
        ],
        "scenarios": [],
    }
    for count in args.files:
        print(f"Running {count:,} files x {args.repeat} commits...", file=sys.stderr)
        report["scenarios"].append(sample(exe, count, args.repeat, args.skip_status))
    output = Path(args.output).resolve()
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"Benchmark saved to {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
