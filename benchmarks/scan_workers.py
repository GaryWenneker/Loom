#!/usr/bin/env python3
"""Checked end-to-end status worker scaling; no isolated microbenchmark claim."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile

from compare_git import command
from compare_publication import content, measured, distribution


def scenario(args, count):
    settings = args.workers
    samples = {str(w): {phase: [] for phase in ('clean', 'eight_changed', 'all_changed')}
               for w in settings}
    paths = [f'files/{n % args.directories:04x}/{n:08d}.bin' for n in range(count)]
    small = set(sorted(range(0, count, max(1, count // 8)))[:8])
    with tempfile.TemporaryDirectory(prefix='loom-scan-workers-') as temporary:
        root = Path(temporary)
        command(root, [args.loom, 'init'])
        for n, name in enumerate(paths):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content(n, args.file_bytes))
        _, output = command(root, [args.loom, 'commit', '-m', 'base'])
        revision = output.decode().strip().split()[-1]
        command(root, [args.loom, 'fsck'])
        for phase in ('clean', 'eight_changed', 'all_changed'):
            changed = set() if phase == 'clean' else (small if phase == 'eight_changed' else set(range(count)))
            if phase != 'clean':
                for n in changed:
                    (root / paths[n]).write_bytes(content(n, args.file_bytes, 1))
            expected = b'On branch main\n' + (
                b''.join(b'M ' + name.encode() + b'\n' for name in sorted(paths[n] for n in changed))
                if changed else b'Working tree clean\n')
            for repeat in range(args.repeat):
                order = settings[repeat % len(settings):] + settings[:repeat % len(settings)]
                for workers in order:
                    metrics, output = measured(root, [args.loom, 'status', '--scan-workers', str(workers)])
                    assert output == expected, 'status byte/order mismatch'
                    samples[str(workers)][phase].append(metrics)
            for n, name in enumerate(paths):
                assert (root / name).read_bytes() == content(n, args.file_bytes, int(n in changed))
            _, output = command(root, [args.loom, 'log', '1'])
            assert output.splitlines()[0] == b'commit ' + revision.encode(), 'status mutated HEAD'
            command(root, [args.loom, 'fsck'])
            print(f'Checked status scaling: {count} files, {phase}', flush=True)
    operations = {w: {p: {metric: distribution([v[metric] for v in values]) for metric in values[0]}
                      for p, values in phases.items()} for w, phases in samples.items()}
    return {'files': count, 'file_bytes': args.file_bytes, 'directories': min(args.directories, count),
            'requested_workers': settings, 'operations': operations,
            'serial_over_workers_p50': {w: {p: operations['1'][p]['wall_ms']['p50'] / values['wall_ms']['p50']
                                           for p, values in phases.items()}
                                        for w, phases in operations.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom', required=True, type=lambda p: str(Path(p).resolve()))
    parser.add_argument('--files', nargs='+', type=int, default=[1000, 10000, 100000])
    parser.add_argument('--workers', nargs='+', type=int, default=[1, 2, 4, 8, 16, 32, 1024])
    parser.add_argument('--repeat', type=int, default=7)
    parser.add_argument('--directories', type=int, default=256)
    parser.add_argument('--file-bytes', type=int, default=256)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not hasattr(os, 'wait4'):
        parser.error('resource measurements require POSIX wait4')
    if (min(args.files) < 1 or max(args.files) > 1000000 or args.repeat < 1 or args.directories < 1
            or not 32 <= args.file_bytes <= 1048576 or min(args.workers) < 1
            or 1 not in args.workers or len(set(args.workers)) != len(args.workers)):
        parser.error('positive unique worker counts including 1, valid repeat/directory/file sizes required')
    report = {'schema_version': 1, 'loom_sha256': hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
              'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'resource_helper_sha256': hashlib.sha256(Path(__file__).with_name('compare_publication.py').read_bytes()).hexdigest(),
              'host': {'platform': platform.platform(), 'cpus': os.cpu_count()},
              'conditions': ['one freshly committed filesystem-provider fixture per size; no Git equivalence claim here',
                             'full CLI startup, full content scan and sorted status output timed; exact output checked',
                             'worker order rotates; seven repeats cover one full seven-setting cycle',
                             'requested limits above eight are capped; actual worker counts depend on CPU/window size',
                             'read-only status; HEAD, full worktree bytes and fsck checked after each phase',
                             'warm uncontrolled OS caches; no cold-disk, remote-network or concurrent-agent measurement',
                             'per-process CPU milliseconds; wait4 RSS KiB may include fork/pre-exec launcher floor',
                             'kernel I/O counters are raw platform units, not portable byte counts'], 'scenarios': []}
    for count in args.files:
        report['scenarios'].append(scenario(args, count))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
