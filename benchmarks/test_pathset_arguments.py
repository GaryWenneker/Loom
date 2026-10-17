#!/usr/bin/env python3
"""Check actual baseline/candidate argv equality for the paired harness mode."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loom", required=True, type=lambda value: str(Path(value).resolve()))
    parser.add_argument("--baseline-loom", type=lambda value: str(Path(value).resolve()))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="loom-pathset-argv-") as temporary:
        root = Path(temporary)
        trace = root / "calls.jsonl"
        for name in ["baseline", "candidate"]:
            wrapper = root / name
            binary = args.baseline_loom if name == "baseline" and args.baseline_loom else args.loom
            wrapper.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, sys\n"
                f"with open({str(trace)!r}, 'a') as output:\n"
                f"    output.write(json.dumps([{name!r}, sys.argv[1:]])+'\\n')\n"
                f"os.execv({binary!r}, [{binary!r}, *sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
        completed = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("pathsets.py")),
             "--loom", str(root / "candidate"), "--baseline-loom", str(root / "baseline"),
             "--files", "16", "--directories", "4", "--changed-paths", "4", "--repeat", "1",
             "--scan-workers", "0", "--baseline-scan-workers", "0", "--baseline-directories",
             "--output", str(root / "result.json")],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        if completed.returncode:
            raise RuntimeError(completed.stderr + completed.stdout)
        calls = {"baseline": [], "candidate": []}
        for line in trace.read_text().splitlines():
            engine, argv = json.loads(line)
            # Exclude seed/bootstrap calls; compare both complete timed flows.
            if (argv[0] == "add" and len(argv) > 4) or (
                argv[0] == "commit" and argv[-1] in ["1", "2"]
            ):
                calls[engine].append(argv)
        if calls["baseline"] != calls["candidate"] or len(calls["baseline"]) != 4:
            raise RuntimeError(f"paired native inputs differ: {calls}")
        directories = [f"files/{index:04x}" for index in range(4)]
        if calls["baseline"][2] != ["add", "--scan-workers", "0", *directories]:
            raise RuntimeError("directory phase did not pass original scopes")
        result = json.loads((root / "result.json").read_text())
        if result["baseline_directory_arguments"] != "same-scopes":
            raise RuntimeError("paired directory mode was not recorded")
        if result["baseline_scan_workers"] != result["loom_scan_workers"]:
            raise RuntimeError("paired worker requests differ")
        print("Actual native add/commit arguments match in both measured phases.")


if __name__ == "__main__":
    main()
