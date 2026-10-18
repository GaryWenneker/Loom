#!/usr/bin/env python3
"""Measure complete portable-tree export and a classified Git diagnostic."""
import argparse
import json
import os
import platform
import random
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def clean_env():
    return {
        **{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
        "LOOM_AUTHOR": "benchmark",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }


def run(root, argv):
    result = subprocess.run(argv, cwd=root, env=clean_env(), capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{argv}: {result.stderr.decode(errors='replace')}")
    return result.stdout


def measured_run(root, argv):
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        started = time.perf_counter_ns()
        process = subprocess.Popen(argv, cwd=root, env=clean_env(), stdout=stdout, stderr=stderr)
        pid, status, usage = os.wait4(process.pid, 0)
        assert pid == process.pid
        process.returncode = os.waitstatus_to_exitcode(status)
        wall_ms = (time.perf_counter_ns() - started) / 1e6
        stdout.seek(0)
        stderr.seek(0)
        output = stdout.read()
        error = stderr.read()
    if process.returncode:
        raise RuntimeError(f"{argv}: {error.decode(errors='replace')}")
    scale = 1024 if platform.system() == "Darwin" else 1
    return {
        "wall_ms": wall_ms,
        "cpu_ms": (usage.ru_utime + usage.ru_stime) * 1000,
        "max_rss_kib": usage.ru_maxrss / scale,
        "inblock": usage.ru_inblock,
        "oublock": usage.ru_oublock,
    }, output


def distribution(samples, key):
    values = sorted(sample[key] for sample in samples)

    def percentile(fraction):
        position = (len(values) - 1) * fraction
        low = int(position)
        high = min(low + 1, len(values) - 1)
        return values[low] + (values[high] - values[low]) * (position - low)

    return {
        "samples": [sample[key] for sample in samples],
        "p50": statistics.median(values),
        "p95": percentile(0.95),
        "p99": percentile(0.99),
    }


def footprint(root):
    logical = allocated = 0
    for path in root.rglob("*"):
        if path.is_file() and not path.is_symlink():
            stat = path.stat()
            logical += stat.st_size
            allocated += getattr(stat, "st_blocks", 0) * 512
    return {"logical_bytes": logical, "allocated_bytes": allocated}


def fixture(root, files, size):
    payload = root / "payload"
    payload.mkdir()
    for index in range(files):
        directory = payload / f"d{index % 64:02d}"
        directory.mkdir(exist_ok=True)
        (directory / f"f{index:06d}").write_bytes(random.Random(index).randbytes(size))


def prepare_loom(root, binary, files, size):
    fixture(root, files, size)
    run(root, [binary, "init"])
    tree = run(root, [binary, "portable-tree", "payload", "--workers", "1"]).decode().strip()
    return tree


def baseline_support(args, workers):
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        tree = prepare_loom(root, args.baseline_loom, 1, 1)
        command = [
            args.baseline_loom,
            "portable-export",
            f"@{tree}",
            str(root / "exported"),
            "--workers",
            str(workers),
        ]
        result = subprocess.run(command, cwd=root, env=clean_env(), capture_output=True)
        return result.returncode == 0, result.returncode, command


def scenario(args, files):
    support = {str(workers): baseline_support(args, workers) for workers in args.workers}
    engines = [f"loom_w{workers}" for workers in args.workers]
    engines += [f"baseline_w{workers}" for workers in args.workers if support[str(workers)][0]]
    engines += ["git"]
    samples = {engine: [] for engine in engines}
    output_footprints = {engine: [] for engine in engines}
    store_footprints = {engine: [] for engine in engines}
    started_argv = {engine: [] for engine in engines}
    observed_roots = {"candidate": set(), "baseline": set()}

    for repeat in range(args.repeat):
        order = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
        for engine in order:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                destination = root / "exported"
                if engine == "git":
                    fixture(root, files, args.file_bytes)
                    payload = root / "payload"
                    run(payload, ["git", "init", "-q", "-b", "main"])
                    run(payload, ["git", "add", "--all"])
                    destination.mkdir()
                    command = ["git", "checkout-index", "--all", f"--prefix={destination}/"]
                    sample, _ = measured_run(payload, command)
                    store_footprints[engine].append(footprint(payload / ".git" / "objects"))
                else:
                    candidate = engine.startswith("loom_w")
                    binary = args.loom if candidate else args.baseline_loom
                    workers = engine.rsplit("w", 1)[1]
                    tree = prepare_loom(root, binary, files, args.file_bytes)
                    observed_roots["candidate" if candidate else "baseline"].add(tree)
                    command = [
                        binary,
                        "portable-export",
                        f"@{tree}",
                        str(destination),
                        "--workers",
                        workers,
                    ]
                    sample, _ = measured_run(root, command)
                    receipt = (destination / ".loom-portable-complete").read_text()
                    assert f"root {tree}\n" in receipt
                    run(root, [binary, "fsck"])
                    store_footprints[engine].append(footprint(root / ".loom" / "objects"))
                samples[engine].append(sample)
                output_footprints[engine].append(footprint(destination))
                started_argv[engine].append(command)

    candidate_roots = observed_roots["candidate"]
    baseline_roots = observed_roots["baseline"]
    assert len(candidate_roots) == 1
    if baseline_roots:
        assert candidate_roots == baseline_roots
    operations = {
        engine: {key: distribution(values, key) for key in values[0]}
        for engine, values in samples.items()
    }
    serial = operations["loom_w1"]["wall_ms"]["p50"]
    candidate_p50 = {
        str(workers): operations[f"loom_w{workers}"]["wall_ms"]["p50"]
        for workers in args.workers
    }
    return {
        "files": files,
        "file_bytes": args.file_bytes,
        "repeat": args.repeat,
        "canonical_root": next(iter(candidate_roots)),
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
        "store_footprints": store_footprints,
        "sequential_over_workers_p50": {
            key: serial / value for key, value in candidate_p50.items()
        },
        "baseline_over_candidate_p50": {
            key: (
                operations[f"baseline_w{key}"]["wall_ms"]["p50"] / candidate_p50[key]
                if f"baseline_w{key}" in operations
                else None
            )
            for key in map(str, args.workers)
        },
        "git_over_candidate_p50_diagnostic": {
            key: operations["git"]["wall_ms"]["p50"] / value
            for key, value in candidate_p50.items()
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
            "candidate and baseline receive identical portable-export tree, destination and explicit worker argument shapes",
            "all actually started argv lists are recorded per scenario and engine",
            "unsupported baseline commands produce no version speed factor",
            "Git checkout-index is a diagnostic subset without live closure verification, typed manifests, fsync or completion receipt",
            "fresh stores and destinations per sample; fixture publication, fsck and Git indexing outside timing",
            "per-process wait4 CPU, peak RSS and kernel block counters; OS caches not flushed",
            "no network, remote cache, concurrent agents, crash injection or power-loss test",
        ],
        "scenarios": [scenario(args, files) for files in args.files],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
