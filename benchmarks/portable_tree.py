#!/usr/bin/env python3
"""Measure complete portable directory publication and a classified Git subset."""
import argparse, json, os, platform, random, resource, statistics, subprocess, tempfile, time
from pathlib import Path

def env():
    return {**{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
            "LOOM_AUTHOR": "benchmark", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}

def run(root, argv):
    result = subprocess.run(argv, cwd=root, env=env(), capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{argv}: {result.stderr.decode(errors='replace')}")
    return result.stdout

def measure(root, commands):
    before = resource.getrusage(resource.RUSAGE_CHILDREN); started = time.perf_counter_ns(); output = b""
    for command in commands: output = run(root, command)
    after = resource.getrusage(resource.RUSAGE_CHILDREN); scale = 1024 if platform.system() == "Darwin" else 1
    return ({"wall_ms": (time.perf_counter_ns() - started) / 1e6,
             "cpu_ms": ((after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)) * 1000,
             "max_rss_kib": after.ru_maxrss / scale, "inblock": after.ru_inblock - before.ru_inblock,
             "oublock": after.ru_oublock - before.ru_oublock}, output)

def distribution(samples, key):
    values = sorted(sample[key] for sample in samples)
    def p(q):
        position = (len(values) - 1) * q; low = int(position); high = min(low + 1, len(values) - 1)
        return values[low] + (values[high] - values[low]) * (position - low)
    return {"samples": [s[key] for s in samples], "p50": statistics.median(values), "p95": p(.95), "p99": p(.99)}

def footprint(root):
    logical = allocated = 0
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat(); logical += stat.st_size; allocated += getattr(stat, "st_blocks", 0) * 512
    return {"logical_bytes": logical, "allocated_bytes": allocated}

def fixture(root, files, size):
    payload = root / "payload"; payload.mkdir()
    for index in range(files):
        directory = payload / f"d{index % 64:02d}"; directory.mkdir(exist_ok=True)
        (directory / f"f{index:06d}").write_bytes(random.Random(index).randbytes(size))

def baseline_supports(args, workers):
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary); run(root, [args.baseline_loom, "init"]); fixture(root, 1, 1)
        result = subprocess.run([args.baseline_loom, "portable-tree", "payload", "--workers", str(workers)], cwd=root, env=env(), capture_output=True)
        return result.returncode == 0, result.returncode

def scenario(args, files):
    support = {str(workers): baseline_supports(args, workers) for workers in args.workers}
    commands = {
        **{f"loom_w{workers}": [[args.loom, "portable-tree", "payload", "--workers", str(workers)]] for workers in args.workers},
        **{f"baseline_w{workers}": [[args.baseline_loom, "portable-tree", "payload", "--workers", str(workers)]] for workers in args.workers},
        "baseline_legacy": [[args.baseline_loom, "portable-tree", "payload"]],
        "git": [["git", "add", "--all", "payload"], ["git", "write-tree"]],
    }
    engines = [f"loom_w{workers}" for workers in args.workers]
    engines += [f"baseline_w{workers}" for workers in args.workers if support[str(workers)][0]]
    engines += ["baseline_legacy", "git"]
    samples = {engine: [] for engine in engines}; footprints = {engine: [] for engine in engines}
    for repeat in range(args.repeat):
        order = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
        for engine in order:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); fixture(root, files, args.file_bytes)
                if engine == "git": run(root, ["git", "init", "-q", "-b", "main"])
                else: run(root, [args.loom if engine.startswith("loom_w") else args.baseline_loom, "init"])
                sample, oid = measure(root, commands[engine]); samples[engine].append(sample)
                if engine == "git":
                    assert run(root, ["git", "cat-file", "-t", oid.decode().strip()]).strip() == b"tree"
                    footprints[engine].append(footprint(root / ".git" / "objects"))
                else:
                    binary = args.loom if engine.startswith("loom_w") else args.baseline_loom
                    run(root, [binary, "fsck"]); assert run(root, commands[engine][0]).strip() == oid.strip()
                    footprints[engine].append(footprint(root / ".loom" / "objects"))
    operations = {engine: {key: distribution(values, key) for key in values[0]} for engine, values in samples.items()}
    serial = operations.get("loom_w1")
    parallel = {key: operations[f"loom_w{key}"]["wall_ms"]["p50"] for key in map(str, args.workers)}
    return {"files": files, "file_bytes": args.file_bytes, "repeat": args.repeat, "commands": commands,
            "baseline_worker_support": {key: {"classification": "measured" if value[0] else "unsupported", "returncode": value[1]} for key, value in support.items()},
            "baseline_legacy_classification": "non-equivalent diagnostic: no explicit worker request",
            "operations": operations, "footprints": footprints,
            "sequential_over_workers_p50": {key: serial["wall_ms"]["p50"] / value for key, value in parallel.items()} if serial else None,
            "baseline_over_candidate_p50": {key: operations[f"baseline_w{key}"]["wall_ms"]["p50"] / operations[f"loom_w{key}"]["wall_ms"]["p50"] if f"baseline_w{key}" in operations else None for key in map(str, args.workers)},
            "git_over_candidate_p50_diagnostic": {key: operations["git"]["wall_ms"]["p50"] / value for key, value in parallel.items()}}

def main():
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument("--loom", required=True); parser.add_argument("--baseline-loom", required=True)
    parser.add_argument("--files", nargs="+", type=int, default=[100, 1000]); parser.add_argument("--file-bytes", type=int, default=4096)
    parser.add_argument("--workers", nargs="+", type=int, default=[1, 2, 4, 0])
    parser.add_argument("--repeat", type=int, default=5); parser.add_argument("--output", required=True, type=Path); args = parser.parse_args()
    report = {"schema_version": 2, "host": {"platform": platform.platform(), "cpus": os.cpu_count()},
              "conditions": ["candidate and baseline receive identical native directory and explicit worker arguments where supported",
                 "the actually started argv lists are recorded under each scenario's commands",
                 "baseline legacy no-worker data is not a pure version-speed comparison",
                 "Git add plus write-tree is a diagnostic subset with different metadata/durability semantics",
                 "fresh stores per sample; verification/idempotent retry outside timing; OS caches not flushed",
                 "no network, remote cache, concurrent agents or power-loss test"],
              "scenarios": [scenario(args, files) for files in args.files]}
    args.output.parent.mkdir(parents=True, exist_ok=True); args.output.write_text(json.dumps(report, indent=2) + "\n"); print(json.dumps(report, indent=2))

if __name__ == "__main__": main()
