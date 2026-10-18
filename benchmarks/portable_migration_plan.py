#!/usr/bin/env python3
"""Measure v1-to-portable-v2 planning/publication and a Git diagnostic."""
import argparse
import json
import os
import platform
import subprocess
import tempfile
from pathlib import Path

from portable_export import clean_env, distribution, fixture, footprint, measured_run, run


def loom_revision(root, binary, files, file_bytes):
    fixture(root, files, file_bytes)
    run(root, [binary, "init"])
    result = run(root, [binary, "commit", "-m", "migration fixture"])
    return result.decode().strip().split()[-1]


def git_revision(root, files, file_bytes):
    fixture(root, files, file_bytes)
    run(root, ["git", "init", "--quiet"])
    run(root, ["git", "add", "payload"])
    run(root, ["git", "-c", "user.name=benchmark", "-c", "user.email=benchmark@example.invalid",
               "commit", "--quiet", "-m", "migration fixture"])


def supports_operation(binary, operation, worker, file_bytes):
    """Probe CLI support in a disposable repository, never the timed one."""
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        revision = loom_revision(root, binary, 1, file_bytes)
        if operation == "verify":
            prepared = subprocess.run(
                [binary, "portable-migrate-checkpoint", f"@{revision}",
                 "--workers", str(worker)],
                cwd=root, env=clean_env(), capture_output=True)
            if prepared.returncode != 0:
                return False
        result = subprocess.run(
            [binary, f"portable-migrate-{operation}", f"@{revision}",
             "--workers", str(worker)],
            cwd=root, env=clean_env(), capture_output=True)
        return result.returncode == 0


def scenario(args, files):
    engines = [f"candidate_w{value}" for value in args.workers]
    engines += [f"baseline_w{value}" for value in args.workers]
    engines += ["git"]
    samples = {engine: [] for engine in engines}
    started = {engine: [] for engine in engines}
    setup_started = {engine: [] for engine in engines}
    footprints = {engine: [] for engine in engines}
    support = {
        str(value): supports_operation(
            args.baseline_loom, args.operation, value, args.file_bytes)
        for value in args.workers
    }
    roots = {str(value): set() for value in args.workers}
    baseline_roots = {str(value): set() for value in args.workers}
    for repeat in range(args.repeat):
        order = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
        for engine in order:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                if engine == "git":
                    git_revision(root, files, args.file_bytes)
                    command = ["git", "fsck", "--full", "--no-reflogs", "HEAD"]
                    sample, _ = measured_run(root, command)
                    footprints[engine].append(footprint(root / ".git"))
                else:
                    baseline = engine.startswith("baseline_")
                    binary = args.baseline_loom if baseline else args.loom
                    worker = engine.rsplit("w", 1)[1]
                    revision = loom_revision(root, binary, files, args.file_bytes)
                    command = [binary, f"portable-migrate-{args.operation}",
                               f"@{revision}", "--workers", worker]
                    if baseline and not support[worker]:
                        footprints[engine].append(footprint(root / ".loom"))
                        continue
                    if args.operation == "verify":
                        setup = [binary, "portable-migrate-checkpoint",
                                 f"@{revision}", "--workers", worker]
                        prepared = subprocess.run(
                            setup, cwd=root, env=clean_env(), capture_output=True)
                        if prepared.returncode != 0:
                            raise RuntimeError(prepared.stderr.decode(errors="replace"))
                        setup_started[engine].append(setup)
                    sample, measured = measured_run(root, command)
                    fields = measured.decode().split()
                    root_field = 2 if args.operation == "plan" else 3
                    portable_root = fields[root_field].rstrip(":")
                    if len(portable_root) != 64 or any(
                            value not in "0123456789abcdef" for value in portable_root):
                        raise RuntimeError(
                            f"invalid portable root in {args.operation} output: {measured!r}")
                    if baseline:
                        baseline_roots[worker].add(portable_root)
                    else:
                        roots[worker].add(portable_root)
                    footprints[engine].append(footprint(root / ".loom"))
                samples[engine].append(sample)
                started[engine].append(command)
    operations = {}
    for engine, values in samples.items():
        operations[engine] = (
            {key: distribution(values, key) for key in values[0]} if values else None
        )
    for worker, values in roots.items():
        assert len(values) == 1
        if support[worker]:
            assert baseline_roots[worker] == values
    serial = operations["candidate_w1"]["wall_ms"]["p50"]
    git = operations["git"]["wall_ms"]["p50"]
    ratios = {}
    versions = {}
    for worker in args.workers:
        key = str(worker)
        candidate = operations[f"candidate_w{worker}"]["wall_ms"]["p50"]
        ratios[key] = serial / candidate
        baseline = operations[f"baseline_w{worker}"]
        versions[key] = None if baseline is None else baseline["wall_ms"]["p50"] / candidate
    return {
        "files": files,
        "file_bytes": args.file_bytes,
        "repeat": args.repeat,
        "candidate_roots": {key: next(iter(value)) for key, value in roots.items()},
        "baseline_roots": {
            key: (next(iter(value)) if value else None)
            for key, value in baseline_roots.items()
        },
        "started_argv": started,
        "setup_argv": setup_started,
        "baseline_support": support,
        "operations": operations,
        "storage_footprints": footprints,
        "sequential_over_workers_p50": ratios,
        "baseline_over_candidate_p50": versions,
        "git_over_candidate_p50_diagnostic": {
            str(worker): git / operations[f"candidate_w{worker}"]["wall_ms"]["p50"]
            for worker in args.workers
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True)
    parser.add_argument("--baseline-loom", required=True)
    parser.add_argument("--files", nargs="+", type=int, default=[100, 1000])
    parser.add_argument("--file-bytes", type=int, default=4096)
    parser.add_argument("--workers", nargs="+", type=int, default=[1, 2, 4, 0])
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument(
        "--operation", choices=["plan", "publish", "checkpoint", "verify"], default="plan")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = {
        "schema_version": 1,
        "host": {"platform": platform.platform(), "cpus": os.cpu_count()},
        "conditions": [
            "fresh repositories and byte-identical payload directories per sample; setup and commit are outside timing",
            f"candidate and previous Loom receive identical portable-migrate-{args.operation} argv and explicit worker requests",
            "CLI support is probed in disposable repositories, never by executing the timed operation first",
            "verify prepares and live-verifies the checkpoint outside timing, then times read-only recomputation and live closure verification",
            "supported candidate and baseline runs must produce identical canonical portable roots",
            "unsupported previous Loom results have no fabricated version factor",
            "Git fsck is a diagnostic verified Git-closure workload, not portable-v2 planning or durable publication",
            "wait4 wall/CPU/peak RSS/block counters; OS caches are not flushed",
            "no network, remote cache, concurrent agents, crash injection, format-state switch or rollback",
        ],
        "operation": args.operation,
        "scenarios": [scenario(args, files) for files in args.files],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
