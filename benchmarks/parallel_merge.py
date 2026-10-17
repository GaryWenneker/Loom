#!/usr/bin/env python3
"""Compare checked bounded Loom text merges with sequential Loom and Git."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile

from compare_git import command
from compare_publication import distribution, measured


def contents(index, side):
    if side == "base":
        return f"base-left-{index}\nshared-{index}\nbase-right-{index}\n".encode()
    if side == "ours":
        return f"ours-left-{index}\nshared-{index}\nbase-right-{index}\n".encode()
    if side == "theirs":
        return f"base-left-{index}\nshared-{index}\ntheirs-right-{index}\n".encode()
    if side == "merged":
        return f"ours-left-{index}\nshared-{index}\ntheirs-right-{index}\n".encode()
    raise ValueError(side)


def write_files(root, files, side):
    for index in range(files):
        path = root / f"files/{index % 64:04x}/{index:08}.txt"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(contents(index, side))


def tree_bytes(root):
    logical = allocated = 0
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat()
            logical += stat.st_size
            allocated += getattr(stat, "st_blocks", 0) * 512
    return logical, allocated


def prepare_git(root, files):
    command(root, ["git", "init", "-q", "-b", "main"])
    for key, value in [
        ("user.name", "Benchmark"),
        ("user.email", "bench@example.invalid"),
        ("core.autocrlf", "false"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
        ("commit.gpgsign", "false"),
    ]:
        command(root, ["git", "config", key, value])
    write_files(root, files, "base")
    command(root, ["git", "add", "--", "files"])
    command(root, ["git", "commit", "-qm", "base"])
    command(root, ["git", "branch", "feature"])
    command(root, ["git", "checkout", "-q", "feature"])
    write_files(root, files, "theirs")
    command(root, ["git", "add", "--", "files"])
    command(root, ["git", "commit", "-qm", "feature"])
    command(root, ["git", "checkout", "-q", "main"])
    write_files(root, files, "ours")
    command(root, ["git", "add", "--", "files"])
    command(root, ["git", "commit", "-qm", "main"])


def verify_files(root, files):
    for index in range(files):
        path = root / f"files/{index % 64:04x}/{index:08}.txt"
        if path.read_bytes() != contents(index, "merged"):
            raise RuntimeError(f"merged bytes differ at {index}")


def scenario(args, files):
    names = [f"loom-{workers}" for workers in args.workers] + ["git"]
    rows = {name: [] for name in names}
    roots = {workers: [] for workers in args.workers}
    for repeat in range(args.repeat):
        order = names[repeat % len(names):] + names[:repeat % len(names)]
        for name in order:
            with tempfile.TemporaryDirectory(prefix="loom-parallel-merge-") as temporary:
                root = Path(temporary)
                if name == "git":
                    prepare_git(root, files)
                    metrics, _ = measured(
                        root, ["git", "merge", "--no-ff", "--no-edit", "feature"]
                    )
                    metrics["merge_wall_ms"] = metrics["wall_ms"]
                    verify_files(root, files)
                    _, status = command(root, ["git", "status", "--porcelain=v1"])
                    if status:
                        raise RuntimeError("Git merge left a dirty worktree")
                    command(root, ["git", "fsck", "--full"])
                    metadata = root / ".git"
                else:
                    workers = int(name.split("-", 1)[1])
                    command(root, [args.binary, "prepare", str(root), str(files)])
                    metrics, output = measured(
                        root, [args.binary, "merge", str(root), str(workers)]
                    )
                    fields = output.decode().strip().split("\t")
                    if len(fields) != 3:
                        raise RuntimeError(f"unexpected merge output: {output!r}")
                    merge_ns, revision, native_root = fields
                    if len(revision) != 64 or len(native_root) != 64:
                        raise RuntimeError("invalid Loom revision/root output")
                    metrics["merge_wall_ms"] = int(merge_ns) / 1_000_000
                    command(
                        root,
                        [args.binary, "verify", str(root), str(files), native_root],
                    )
                    roots[workers].append(native_root)
                    metadata = root / ".loom"
                logical, allocated = tree_bytes(metadata)
                metrics["metadata_logical_bytes"] = logical
                metrics["metadata_allocated_bytes"] = allocated
                rows[name].append(metrics)
        if len({roots[workers][-1] for workers in args.workers}) != 1:
            raise RuntimeError("sequential and parallel Loom roots differ")
        print(f"Checked {files} files: repeat {repeat + 1}/{args.repeat}", flush=True)

    operations = {
        name: {
            metric: distribution([sample[metric] for sample in samples])
            for metric in samples[0]
        }
        for name, samples in rows.items()
    }
    serial = operations[f"loom-{args.workers[0]}"]["merge_wall_ms"]["p50"]
    parallel = operations[f"loom-{args.workers[-1]}"]["merge_wall_ms"]["p50"]
    git = operations["git"]["merge_wall_ms"]["p50"]
    return {
        "files": files,
        "independently_text_merged_files": files,
        "operations": operations,
        "canonical_loom_roots": roots,
        "worker_one_over_parallel_p50": serial / parallel,
        "git_over_parallel_loom_p50": git / parallel,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=lambda value: str(Path(value).resolve()))
    parser.add_argument("--files", nargs="+", type=int, default=[64, 512])
    parser.add_argument("--workers", nargs="+", type=int, default=[1, 2])
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not hasattr(os, "wait4"):
        parser.error("per-process resource measurements require POSIX wait4")
    if min(args.files) < 1 or min(args.workers) < 1 or args.repeat < 1:
        parser.error("positive files/workers/repeat required")
    if len(set(args.workers)) != len(args.workers) or args.workers[0] != 1:
        parser.error("workers must be unique and start with sequential worker 1")

    binary = Path(args.binary)
    report = {
        "schema_version": 1,
        "git_version": subprocess.check_output(["git", "--version"], text=True).strip(),
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
        "metric_units": {
            "merge_wall_ms": "milliseconds; merge call/command only",
            "wall_ms": "milliseconds; measured merge process including startup",
            "cpu_ms": "milliseconds; measured merge process",
            "max_rss_kib": "KiB; measured merge process",
            "rusage_inblock": "kernel counter units, not bytes",
            "rusage_oublock": "kernel counter units, not bytes",
            "metadata_logical_bytes": "bytes",
            "metadata_allocated_bytes": "st_blocks*512 bytes",
        },
        "conditions": [
            "fresh repository per engine/sample; fixture creation and validation outside timing",
            "identical independent non-overlapping line edits on both branches",
            "Loom worker order and engine order rotate",
            "every Loom result verifies all bytes, clean status, full fsck and canonical root equality",
            "every Git result verifies all bytes, clean status and full fsck",
            "Loom explicit workers remain capped by CPU, eight and provider read hint",
            "OS caches are not flushed; no controlled cold-cache evidence",
            "local filesystem only; network and remote cache use are not measured",
            "single merge process; no concurrent-agent or fault-injection evidence",
        ],
        "scenarios": [],
    }
    for files in args.files:
        report["scenarios"].append(scenario(args, files))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        latest = report["scenarios"][-1]
        print(json.dumps({
            "files": files,
            "worker_one_over_parallel_p50": latest["worker_one_over_parallel_p50"],
            "git_over_parallel_loom_p50": latest["git_over_parallel_loom_p50"],
        }), flush=True)


if __name__ == "__main__":
    main()
