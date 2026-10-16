#!/usr/bin/env python3
"""Run isolated Loom workspaces concurrently against one shared object provider."""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--backend', choices=['sqlite', 'filesystem'], default='sqlite')
    parser.add_argument('--agents', type=int, default=10)
    parser.add_argument('--commits', type=int, default=10)
    args = parser.parse_args()
    if args.agents < 1 or args.commits < 1:
        parser.error('agents and commits must be positive')
    binary = args.binary.resolve(strict=True)
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='loom-shared-object-stress-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        env.pop('LOOM_OBJECT_DIR', None)
        env.pop('LOOM_OBJECT_DB', None)
        env['LOOM_AUTHOR'] = 'shared-storage-stress'
        variable = 'LOOM_OBJECT_DB' if args.backend == 'sqlite' else 'LOOM_OBJECT_DIR'
        env[variable] = str(root / ('objects.sqlite' if args.backend == 'sqlite' else 'objects'))

        def run(workspace, *command):
            result = subprocess.run([str(binary), *command], cwd=workspace, env=env,
                                    capture_output=True, timeout=60)
            if result.returncode:
                raise RuntimeError(f'{workspace.name} {command}: '
                                   f'{result.stderr.decode(errors="replace")}')
            return result.stdout

        def actor(number):
            workspace = root / f'workspace-{number}'
            workspace.mkdir()
            run(workspace, 'init')
            for revision in range(args.commits):
                (workspace / 'file').write_text(f'workspace={number}, revision={revision}\n')
                run(workspace, 'add', 'file')
                run(workspace, 'commit', '-m', f'actor {number} revision {revision}')
            expected = f'workspace={number}, revision={args.commits - 1}\n'.encode()
            if run(workspace, 'cat', 'main', 'file') != expected:
                raise RuntimeError(f'workspace {number}: final snapshot mismatch')
            history = run(workspace, 'log', str(args.commits + 1))
            if sum(line.startswith(b'commit ') for line in history.splitlines()) != args.commits:
                raise RuntimeError(f'workspace {number}: lost acknowledged commit')
            return workspace

        with concurrent.futures.ThreadPoolExecutor(max_workers=args.agents) as pool:
            workspaces = list(pool.map(actor, range(args.agents)))
        for workspace in workspaces:
            run(workspace, 'fsck')
        print(json.dumps({'backend': args.backend, 'concurrent_workspaces': args.agents,
                          'acknowledged_commits': args.agents * args.commits,
                          'history_and_snapshot_checks': len(workspaces),
                          'fsck_passed': len(workspaces),
                          'elapsed_seconds': round(time.monotonic() - started, 2)}))


if __name__ == '__main__':
    main()
