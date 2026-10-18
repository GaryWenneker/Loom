#!/usr/bin/env python3
"""Measure complete portable workspace verification and a classified Git diagnostic."""
import argparse
import json
import os
import platform
import shutil
import subprocess
import tempfile
from pathlib import Path

from portable_export import clean_env, distribution, fixture, footprint, measured_run, prepare_loom, run


def baseline_support(args, workers):
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        run(root, [args.baseline_loom, "init"])
        command = [
            args.baseline_loom,
            "portable-verify",
            "@" + "00" * 32,
            str(root / "workspace"),
            "--workers",
            str(workers),
        ]
        result = subprocess.run(command, cwd=root, env=clean_env(), capture_output=True)
        return result.returncode == 0, result.returncode, command


def scenario(args, files):
    support = {str(workers): baseline_support(args, workers) for workers in args.workers}
    engines = [f"loom_w{workers}" for workers in args.workers] + ["git"]
    samples = {engine: [] for engine in engines}
    started_argv = {engine: [] for engine in engines}
    output_footprints = {engine: [] for engine in engines}
    roots = set()
    for repeat in range(args.repeat):
        order = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
        for engine in order:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                if engine == "git":
                    fixture(root, files, args.file_bytes)
                    shutil.copytree(root / "payload", root / "workspace")
                    command = [
                        "git",
                        "diff",
                        "--no-index",
                        "--quiet",
                        str(root / "payload"),
                        str(root / "workspace"),
                    ]
                    sample, _ = measured_run(root, command)
                else:
                    tree = prepare_loom(root, args.loom, files, args.file_bytes)
                    roots.add(tree)
                    workspace = root / "workspace"
                    run(
                        root,
                        [args.loom, "portable-export", f"@{tree}", str(workspace), "--workers", "1"],
                    )
                    workers = engine.rsplit("w", 1)[1]
                    command = [
                        args.loom,
                        "portable-verify",
                        f"@{tree}",
                        str(workspace),
                        "--workers",
                        workers,
                    ]
                    sample, _ = measured_run(root, command)
                samples[engine].append(sample)
                started_argv[engine].append(command)
                output_footprints[engine].append(footprint(root / "workspace"))
    assert len(roots) == 1
    operations = {
        engine: {key: distribution(values, key) for key in values[0]}
        for engine, values in samples.items()
    }
    serial = operations["loom_w1"]["wall_ms"]["p50"]
    candidate = {
        str(workers): operations[f"loom_w{workers}"]["wall_ms"]["p50"]
        for workers in args.workers
    }
    git = operations["git"]["wall_ms"]["p50"]
    return {
        "files": files,
        "file_bytes": args.file_bytes,
        "repeat": args.repeat,
        "canonical_root": next(iter(roots)),
        "started_argv": started_argv,
        "baseline_worker_support": {
            key: {
                "classification": "measured" if value[0] else "unsupported",
                "returncode": value[1],
                "probe_argv": value[2],
            }
            for key, value in support.items()
        },
        "operations": operations,
        "output_footprints": output_footprints,
        "sequential_over_workers_p50": {
            key: serial / value for key, value in candidate.items()
        },
        "git_over_candidate_p50_diagnostic": {
            key: git / value for key, value in candidate.items()
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
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = {
        "schema_version": 1,
        "host": {"platform": platform.platform(), "cpus": os.cpu_count()},
        "conditions": [
            "fresh canonical store and exported workspace per sample; setup is outside timing",
            "candidate worker argv is recorded; previous Loom receives the same command shape and unsupported yields no version factor",
            "Git diff --no-index is a diagnostic directory comparison without canonical object closure, receipt or portable-mode contract",
            "per-process wait4 wall/CPU/peak RSS/block counters; OS caches are not flushed",
            "no network, remote cache, concurrent agents, mutation race injection or power-loss test",
        ],
        "scenarios": [scenario(args, files) for files in args.files],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
