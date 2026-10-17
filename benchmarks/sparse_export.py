#!/usr/bin/env python3
"""Compare sparse snapshot materialization with prior Loom and Git."""
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


def env():
    return {
        **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
        "LOOM_AUTHOR": "benchmark",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }


def run(root, argv, data=None):
    result = subprocess.run(argv, cwd=root, env=env(), input=data, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{argv}: {result.stderr[:2000].decode(errors='replace')}")
    return result.stdout


def content(index, size):
    head = f"file={index:08d};".encode()
    return (head + b"sparse\x00payload\n" * (size // 15 + 1))[:size]


def parse_loom(output, expected):
    offset = 0
    values = []
    for request_id, wanted in enumerate(expected):
        end = output.index(b"\n", offset)
        header = json.loads(output[offset:end])
        offset = end + 1
        assert header["id"] == request_id and header["kind"] == "blob" and header["ok"]
        size = header["size"]
        value = output[offset : offset + size]
        assert value == wanted
        values.append(value)
        offset += size
        assert output[offset : offset + 1] == b"\n"
        offset += 1
    assert offset == len(output)
    return values


def write_files(destination, paths, values):
    destination.mkdir()
    for path, value in zip(paths, values):
        target = destination / path
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open("xb") as output:
            output.write(value)
            output.flush()
            os.fsync(output.fileno())


def usage_delta(before, after):
    return {
        "cpu_ms": ((after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)) * 1000,
        "max_rss_kib": after.ru_maxrss / (1024 if platform.system() == "Darwin" else 1),
        "rusage_inblock": after.ru_inblock - before.ru_inblock,
        "rusage_oublock": after.ru_oublock - before.ru_oublock,
    }


def measure(action):
    self_before = resource.getrusage(resource.RUSAGE_SELF)
    child_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter_ns()
    action()
    wall_ms = (time.perf_counter_ns() - started) / 1e6
    self_after = resource.getrusage(resource.RUSAGE_SELF)
    child_after = resource.getrusage(resource.RUSAGE_CHILDREN)
    own = usage_delta(self_before, self_after)
    child = usage_delta(child_before, child_after)
    return {
        "wall_ms": wall_ms,
        "cpu_ms": own["cpu_ms"] + child["cpu_ms"],
        "max_rss_kib": max(own["max_rss_kib"], child["max_rss_kib"]),
        "rusage_inblock": own["rusage_inblock"] + child["rusage_inblock"],
        "rusage_oublock": own["rusage_oublock"] + child["rusage_oublock"],
    }


def distribution(values):
    ordered = sorted(values)
    def percentile(q):
        position = (len(ordered) - 1) * q
        low = int(position)
        high = min(low + 1, len(ordered) - 1)
        return ordered[low] + (ordered[high] - ordered[low]) * (position - low)
    return {"samples": values, "p50": statistics.median(values), "p95": percentile(.95), "p99": percentile(.99)}


def footprint(root):
    logical = allocated = 0
    for path in root.rglob("*"):
        if path.is_file():
            stat = path.stat()
            logical += stat.st_size
            allocated += getattr(stat, "st_blocks", 0) * 512
    return [logical, allocated]


def selected_paths(all_paths, prefixes):
    return [path for path in all_paths if any(path == p or path.startswith(p + "/") for p in prefixes)]


def verify(destination, paths, expected, marker=False):
    actual = sorted(
        str(path.relative_to(destination)).replace(os.sep, "/")
        for path in destination.rglob("*")
        if path.is_file() and path.name != ".loom-sparse-complete"
    )
    assert actual == paths
    for path, value in zip(paths, expected):
        assert (destination / path).read_bytes() == value
    assert (destination / ".loom-sparse-complete").is_file() == marker


def scenario(args, count, mode):
    paths = [f"files/d{index % 64:02d}/f{index:08d}.bin" for index in range(count)]
    values = [content(index, args.file_bytes) for index in range(count)]
    prefixes = paths[:8] if mode == "files" else [f"files/d{index:02d}" for index in range(8)]
    wanted_paths = selected_paths(paths, prefixes)
    expected_by_path = dict(zip(paths, values))
    wanted = [expected_by_path[path] for path in wanted_paths]
    samples = {name: [] for name in ("candidate", "baseline", "git")}
    footprints = {name: [] for name in samples}

    for repeat in range(args.repeat):
        with tempfile.TemporaryDirectory(prefix="loom-sparse-export-") as temporary:
            root = Path(temporary)
            workspace = root / "source"
            workspace.mkdir()
            run(workspace, [args.baseline_loom, "init"])
            for path, value in zip(paths, values):
                target = workspace / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(value)
            revision = run(workspace, [args.baseline_loom, "commit", "-m", "fixture"]).decode().split()[-1]
            run(workspace, ["git", "init", "-q", "-b", "main"])
            run(workspace, ["git", "config", "user.name", "Benchmark"])
            run(workspace, ["git", "config", "user.email", "bench@example.invalid"])
            run(workspace, ["git", "add", "--", "files"])
            run(workspace, ["git", "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"])

            order = ["candidate", "baseline", "git"]
            order = order[repeat % 3 :] + order[: repeat % 3]
            for engine in order:
                destination = root / engine
                if engine == "candidate":
                    action = lambda: run(workspace, [args.loom, "sparse-export", "@" + revision, str(destination), *prefixes])
                elif engine == "baseline":
                    def action():
                        listed = []
                        for prefix in prefixes:
                            output = run(workspace, [args.baseline_loom, "files", "@" + revision, prefix, "--null"])
                            listed.extend(item.decode() for item in output.split(b"\0") if item)
                        listed = sorted(set(listed))
                        requests = b"".join(
                            json.dumps({"id": index, "op": "cat", "path": path}).encode() + b"\n"
                            for index, path in enumerate(listed)
                        )
                        output = run(workspace, [args.baseline_loom, "query", "--batch", "@" + revision], requests)
                        write_files(destination, listed, parse_loom(output, [expected_by_path[p] for p in listed]))
                else:
                    def action():
                        listing = run(workspace, ["git", "ls-tree", "-r", "--name-only", "-z", "HEAD", "--", *prefixes])
                        git_paths = [item.decode() for item in listing.split(b"\0") if item]
                        destination.mkdir()
                        run(workspace, ["git", "checkout-index", "-z", "--stdin", "--prefix=" + str(destination) + os.sep], b"\0".join(path.encode() for path in git_paths) + b"\0")
                samples[engine].append(measure(action))
                verify(destination, wanted_paths, wanted, engine == "candidate")
                footprints[engine].append(footprint(destination))

    operations = {
        engine: {
            metric: distribution([sample[metric] for sample in engine_samples])
            for metric in engine_samples[0]
        }
        for engine, engine_samples in samples.items()
    }
    return {
        "tracked_files": count,
        "selection": mode,
        "selected_files": len(wanted_paths),
        "operations": operations,
        "footprints": footprints,
        "baseline_over_candidate_p50": operations["baseline"]["wall_ms"]["p50"] / operations["candidate"]["wall_ms"]["p50"],
        "git_over_candidate_p50": operations["git"]["wall_ms"]["p50"] / operations["candidate"]["wall_ms"]["p50"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True, type=lambda p: str(Path(p).resolve()))
    parser.add_argument("--baseline-loom", required=True, type=lambda p: str(Path(p).resolve()))
    parser.add_argument("--files", nargs="+", type=int, default=[256, 4096])
    parser.add_argument("--file-bytes", type=int, default=256)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = {
        "schema_version": 1,
        "host": {"platform": platform.platform(), "cpus": os.cpu_count()},
        "conditions": [
            "candidate, prior Loom compatibility client and Git materialize identical selected bytes",
            "candidate uses one sparse-export process; prior Loom uses public files/query commands",
            "Git uses ls-tree plus checkout-index; command startup and framing differ",
            "fresh repository and destination per engine/sample; setup and verification are outside timing",
            "file fsync is retained; directory-entry power-loss durability is not certified",
            "OS caches are not flushed; network, remote cache and concurrent agents are unmeasured",
            "resource counters include the Python compatibility client and its child processes",
        ],
        "scenarios": [],
    }
    for count in args.files:
        for mode in ("files", "directories"):
            report["scenarios"].append(scenario(args, count, mode))
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(f"checked {count} {mode}", flush=True)


if __name__ == "__main__":
    main()
