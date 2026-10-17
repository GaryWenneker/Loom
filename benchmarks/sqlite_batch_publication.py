#!/usr/bin/env python3
"""Compare complete SQLite-backed Loom commit workflows with prior Loom and Git."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import threading
import time

from compare_git import parse_git_batch, parse_loom_batch, summary


def content(index, size, version):
    header = f"file={index:08d};version={version};".encode()
    return (header + (b"\x00\xffsqlite-batch\n" * (size // 15 + 1)))[:size]


def environment(database=None):
    env = {
        **{
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_") and key not in ("LOOM_OBJECT_DIR", "LOOM_OBJECT_DB")
        },
        "LOOM_AUTHOR": "benchmark",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }
    if database is not None:
        env["LOOM_OBJECT_DB"] = str(database)
    return env


def command(root, argv, database=None, stdin=None):
    result = subprocess.run(
        argv,
        cwd=root,
        env=environment(database),
        input=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"{argv}: {result.stderr[:2000].decode(errors='replace')}")
    return result.stdout


def measured(root, argv, database=None):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        started = time.perf_counter_ns()
        with subprocess.Popen(
            argv,
            cwd=root,
            env=environment(database),
            stdout=output,
            stderr=errors,
        ) as child:
            timer = threading.Timer(600, child.kill)
            timer.daemon = True
            timer.start()
            try:
                _, status, usage = os.wait4(child.pid, 0)
                elapsed = (time.perf_counter_ns() - started) / 1e6
                child.returncode = os.waitstatus_to_exitcode(status)
            finally:
                timer.cancel()
                timer.join()
            if child.returncode:
                errors.seek(0)
                raise RuntimeError(f"{argv}: {errors.read(2000).decode(errors='replace')}")
        output.seek(0)
        stdout = output.read()
    return {
        "wall_ms": elapsed,
        "cpu_ms": (usage.ru_utime + usage.ru_stime) * 1000,
        "max_rss_kib": usage.ru_maxrss / (1024 if platform.system() == "Darwin" else 1),
        "rusage_inblock": usage.ru_inblock,
        "rusage_oublock": usage.ru_oublock,
    }, stdout


def distribution(values):
    return {key.removesuffix("_ms"): value for key, value in summary(values).items()}


def footprint(paths):
    logical = allocated = 0
    for root in paths:
        candidates = root.rglob("*") if root.is_dir() else [root]
        for path in candidates:
            if path.is_file():
                stat = path.stat()
                logical += stat.st_size
                allocated += getattr(stat, "st_blocks", 0) * 512
    return logical, allocated


def verify_loom(binary, workspace, database, paths, expected):
    revision = command(workspace, [binary, "log", "1"], database).decode().split()[1]
    listing = command(workspace, [binary, "files", "@" + revision, "--null"], database)
    assert listing == b"".join(path.encode() + b"\0" for path in sorted(paths))
    requests = b"".join(
        json.dumps({"id": index, "op": "cat", "path": path}).encode() + b"\n"
        for index, path in enumerate(paths)
    )
    output = command(workspace, [binary, "query", "--batch", "@" + revision], database, requests)
    parse_loom_batch(output, expected)
    command(workspace, [binary, "fsck"], database)
    stats = command(
        workspace,
        [binary, "query", "--batch", "@" + revision],
        database,
        b'{"id":0,"op":"stats"}\n',
    )
    return json.loads(stats)["root"]


def scenario(args, files):
    names = ["baseline", "loom", "git"]
    phases = ["initial", "incremental"]
    samples = {name: {phase: [] for phase in phases} for name in names}
    roots = {name: [] for name in names if name != "git"}
    footprints = {name: [] for name in names}
    paths = [f"files/{index % 32:02x}/{index:08d}.bin" for index in range(files)]
    changed = list(range(min(8, files)))

    for repeat in range(args.repeat):
        order = names[repeat % len(names) :] + names[: repeat % len(names)]
        for name in order:
            with tempfile.TemporaryDirectory(prefix="loom-sqlite-batch-") as temporary:
                root = Path(temporary)
                workspace = root / "workspace"
                workspace.mkdir()
                database = root / "objects.sqlite" if name != "git" else None
                binary = args.baseline_loom if name == "baseline" else args.loom
                if name == "git":
                    command(workspace, ["git", "init", "-q", "-b", "main"])
                    command(workspace, ["git", "config", "user.name", "Benchmark"])
                    command(workspace, ["git", "config", "user.email", "bench@example.invalid"])
                    command(workspace, ["git", "config", "gc.auto", "0"])
                    command(workspace, ["git", "config", "maintenance.auto", "false"])
                else:
                    command(root, [binary, "init", str(workspace), "--object-db", str(database)])

                for index, path in enumerate(paths):
                    target = workspace / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(content(index, args.file_bytes, 0))

                for phase in phases:
                    version = int(phase == "incremental")
                    if version:
                        for index in changed:
                            (workspace / paths[index]).write_bytes(
                                content(index, args.file_bytes, version)
                            )
                    if name == "git":
                        started = time.perf_counter_ns()
                        add, _ = measured(workspace, ["git", "add", "--", "files"])
                        commit, _ = measured(
                            workspace,
                            ["git", "-c", "commit.gpgsign=false", "commit", "-qm", phase],
                        )
                        metrics = {
                            key: max(add[key], commit[key])
                            if key == "max_rss_kib"
                            else add[key] + commit[key]
                            for key in add
                        }
                        metrics["wall_ms"] = (time.perf_counter_ns() - started) / 1e6
                    else:
                        metrics, _ = measured(
                            workspace, [binary, "commit", "-m", phase], database
                        )
                    samples[name][phase].append(metrics)

                expected = [
                    content(index, args.file_bytes, int(index in changed))
                    for index in range(files)
                ]
                if name == "git":
                    revision = command(workspace, ["git", "rev-parse", "HEAD"]).decode().strip()
                    listing = command(
                        workspace, ["git", "ls-tree", "-r", "--name-only", "-z", revision]
                    )
                    assert listing == b"".join(path.encode() + b"\0" for path in sorted(paths))
                    query = b"".join((revision + ":" + path + "\n").encode() for path in paths)
                    parse_git_batch(command(workspace, ["git", "cat-file", "--batch"], stdin=query), expected)
                    command(workspace, ["git", "fsck", "--full"])
                    footprints[name].append(footprint([workspace / ".git"]))
                else:
                    roots[name].append(
                        verify_loom(binary, workspace, database, paths, expected)
                    )
                    footprints[name].append(footprint([workspace / ".loom", database]))
        assert roots["baseline"][-1] == roots["loom"][-1]
        print(f"Checked {files} SQLite-backed files: repeat {repeat + 1}/{args.repeat}", flush=True)

    operations = {
        name: {
            phase: {
                metric: distribution([sample[metric] for sample in values])
                for metric in values[0]
            }
            for phase, values in phase_samples.items()
        }
        for name, phase_samples in samples.items()
    }
    return {
        "files": files,
        "changed_files": len(changed),
        "operations": operations,
        "canonical_roots": roots,
        "metadata_footprints": footprints,
        "baseline_over_loom_p50": {
            phase: operations["baseline"][phase]["wall_ms"]["p50"]
            / operations["loom"][phase]["wall_ms"]["p50"]
            for phase in phases
        },
        "git_over_loom_p50": {
            phase: operations["git"][phase]["wall_ms"]["p50"]
            / operations["loom"][phase]["wall_ms"]["p50"]
            for phase in phases
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True, type=lambda path: str(Path(path).resolve()))
    parser.add_argument("--baseline-loom", required=True, type=lambda path: str(Path(path).resolve()))
    parser.add_argument("--files", nargs="+", type=int, default=[32, 256])
    parser.add_argument("--file-bytes", type=int, default=256)
    parser.add_argument("--repeat", type=int, default=7)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not hasattr(os, "wait4"):
        parser.error("POSIX wait4 is required")
    if min(args.files) < 1 or args.repeat < 1 or not 32 <= args.file_bytes <= 1024 * 1024:
        parser.error("positive files/repeat and file bytes 32..1048576 required")

    report = {
        "schema_version": 1,
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
        "git_version": subprocess.check_output(["git", "--version"], text=True).strip(),
        "loom_sha256": hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
        "baseline_loom_sha256": hashlib.sha256(Path(args.baseline_loom).read_bytes()).hexdigest(),
        "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "metric_units": {
            "wall_ms": "milliseconds; complete commit command or Git add+commit",
            "cpu_ms": "milliseconds",
            "max_rss_kib": "KiB",
            "rusage_inblock": "kernel counter units, not bytes",
            "rusage_oublock": "kernel counter units, not bytes",
            "metadata_footprints": "logical bytes, allocated bytes",
        },
        "conditions": [
            "fresh repository per engine/sample; identical bytes and nested paths",
            "Loom uses external SQLite objects with WAL and synchronous=FULL",
            "candidate and baseline run complete commit commands; Git runs add+commit",
            "fixture creation and byte/listing/fsck verification are outside timing",
            "initial commit is the large changeset; incremental commit changes eight files",
            "engine order rotates and every Loom pair must produce identical canonical roots",
            "OS caches are not flushed; no controlled cold-cache evidence",
            "local filesystem only; network, remote cache and multi-agent traffic are unmeasured",
            "no durability setting is relaxed and no power-loss certification is claimed",
        ],
        "scenarios": [],
    }
    for files in args.files:
        report["scenarios"].append(scenario(args, files))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
