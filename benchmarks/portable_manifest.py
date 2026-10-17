#!/usr/bin/env python3
"""Measure durable portable-manifest publication and a classified Git subset."""
import argparse
import json
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import tempfile
import time


def environment():
    return {
        **{key: value for key, value in os.environ.items() if not key.startswith("GIT_")},
        "LOOM_AUTHOR": "benchmark",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }


def run(root, argv):
    result = subprocess.run(argv, cwd=root, env=environment(), capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{argv}: {result.stderr.decode(errors='replace')}")
    return result.stdout


def usage_delta(before, after):
    rss_scale = 1024 if platform.system() == "Darwin" else 1
    return {
        "cpu_ms": ((after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)) * 1000,
        "max_rss_kib": after.ru_maxrss / rss_scale,
        "inblock": after.ru_inblock - before.ru_inblock,
        "oublock": after.ru_oublock - before.ru_oublock,
    }


def measure(root, argv):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter_ns()
    output = run(root, argv)
    wall_ms = (time.perf_counter_ns() - started) / 1e6
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {"wall_ms": wall_ms, **usage_delta(before, after)}, output


def distribution(samples, key):
    values = sorted(sample[key] for sample in samples)
    def percentile(q):
        position = (len(values) - 1) * q
        low = int(position)
        high = min(low + 1, len(values) - 1)
        return values[low] + (values[high] - values[low]) * (position - low)
    return {
        "samples": [sample[key] for sample in samples],
        "p50": statistics.median(values),
        "p95": percentile(.95),
        "p99": percentile(.99),
    }


def footprint(root):
    logical = allocated = 0
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat()
            logical += stat.st_size
            allocated += getattr(stat, "st_blocks", 0) * 512
    return {"logical_bytes": logical, "allocated_bytes": allocated}


def payload(size):
    seed = b"loom portable manifest benchmark\x00"
    return (seed * (size // len(seed) + 1))[:size]


def write_payload(path, size):
    block = payload(min(size, 1024 * 1024))
    with path.open("wb") as output:
        remaining = size
        while remaining:
            part = block[:min(remaining, len(block))]
            output.write(part)
            remaining -= len(part)


def baseline_supports(args, size):
    with tempfile.TemporaryDirectory(prefix="loom-v2-baseline-probe-") as temporary:
        root = Path(temporary)
        run(root, [args.baseline_loom, "init"])
        write_payload(root / "payload", size)
        probe = subprocess.run(
            [args.baseline_loom, "portable-manifest", "regular", "payload"],
            cwd=root, env=environment(), capture_output=True,
        )
        return probe.returncode == 0, probe.returncode


def scenario(args, size):
    baseline_supported, baseline_returncode = baseline_supports(args, size)
    engines = ["loom", "baseline_loom", "git"] if baseline_supported else ["loom", "git"]
    samples = {engine: [] for engine in engines}
    footprints = {engine: [] for engine in engines}
    commands = {
        "loom": [args.loom, "portable-manifest", "regular", "payload"],
        "baseline_loom": [args.baseline_loom, "portable-manifest", "regular", "payload"],
        "git": ["git", "hash-object", "-w", "payload"],
    }
    repeats = args.large_repeat if size > 64 * 1024 * 1024 else args.repeat
    for repeat in range(repeats):
        order = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
        for engine in order:
            with tempfile.TemporaryDirectory(prefix="loom-v2-manifest-") as temporary:
                root = Path(temporary)
                write_payload(root / "payload", size)
                if engine in ("loom", "baseline_loom"):
                    binary = args.loom if engine == "loom" else args.baseline_loom
                    run(root, [binary, "init"])
                else:
                    run(root, ["git", "init", "-q", "-b", "main"])
                sample, object_id = measure(root, commands[engine])
                samples[engine].append(sample)
                object_id = object_id.decode().strip()
                if engine in ("loom", "baseline_loom"):
                    binary = args.loom if engine == "loom" else args.baseline_loom
                    run(root, [binary, "fsck"])
                    assert run(root, commands[engine]).decode().strip() == object_id
                    footprints[engine].append(footprint(root / ".loom" / "objects"))
                else:
                    assert int(run(root, ["git", "cat-file", "-s", object_id])) == size
                    footprints[engine].append(footprint(root / ".git" / "objects"))
    operations = {
        engine: {
            metric: distribution(engine_samples, metric)
            for metric in engine_samples[0]
        }
        for engine, engine_samples in samples.items()
    }
    return {
        "bytes": size,
        "repeat": repeats,
        "commands": commands,
        "baseline": {
            "classification": "measured" if baseline_supported else "unsupported",
            "returncode": baseline_returncode,
        },
        "operations": operations,
        "footprints": footprints,
        "baseline_over_candidate_p50": (
            operations["baseline_loom"]["wall_ms"]["p50"]
            / operations["loom"]["wall_ms"]["p50"]
            if baseline_supported else None
        ),
        "git_over_loom_p50_diagnostic": (
            operations["git"]["wall_ms"]["p50"] / operations["loom"]["wall_ms"]["p50"]
        ),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True, type=lambda path: str(Path(path).resolve()))
    parser.add_argument("--baseline-loom", required=True, type=lambda path: str(Path(path).resolve()))
    parser.add_argument(
        "--bytes", nargs="+", type=int,
        default=[1024, 8 * 1024 * 1024, 68 * 1024 * 1024],
    )
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--large-repeat", type=int, default=3)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = {
        "schema_version": 2,
        "host": {"platform": platform.platform(), "cpus": os.cpu_count()},
        "conditions": [
            "candidate publishes fixed 4 MiB chunks plus typed v2 manifest and runs Loom fsck outside timing",
            "candidate and previous Loom receive identical commands; unsupported payload sizes receive no version factor",
            "Git hash-object publishes equivalent bytes but no Loom portable metadata or chunk closure",
            "Git default durability can differ from Loom file fsync, so its ratio is diagnostic, not a pure speed claim",
            "fresh object store per sample; verification and retry are outside timing; OS caches are not flushed",
            "no network, remote cache, concurrent agents, migration, ref CAS or power-loss test is included",
        ],
        "scenarios": [scenario(args, size) for size in args.bytes],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
