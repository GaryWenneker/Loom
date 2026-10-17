#!/usr/bin/env python3
"""Confirm fresh-session grouped reads using many paired native samples."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import tempfile

from compare_git import command, content, parse_loom_multi, summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom', required=True, type=lambda s: str(Path(s).resolve()))
    parser.add_argument('--baseline-loom', required=True, type=lambda s: str(Path(s).resolve()))
    parser.add_argument('--files', type=int, default=100000)
    parser.add_argument('--directories', type=int, default=256)
    parser.add_argument('--repeat', type=int, default=50)
    parser.add_argument('--requests', type=int, default=1000)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if min(args.files, args.directories, args.repeat, args.requests) < 1 or args.requests > 4096:
        parser.error('positive sizes and at most 4096 requests are required')
    def relative(n):
        return f'files/{n % args.directories:02x}/{n:08d}.bin'
    samples = {'baseline': [], 'loom': []}
    with tempfile.TemporaryDirectory(prefix='loom-native-pairs-') as temporary:
        root = Path(temporary)
        command(root, [args.baseline_loom, 'init'])
        for n in range(args.files):
            path = root/relative(n)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content(n))
        command(root, [args.baseline_loom, 'commit', '-m', 'same tracked fixture'])
        for n in range(10):
            (root/'noise').write_text(f'change {n}\n')
            command(root, [args.baseline_loom, 'add', 'noise'])
            _, output = command(root, [args.baseline_loom, 'commit', '-m', f'noise {n}'])
        revision = output.decode().strip().split()[-1]
        for repeat in range(args.repeat):
            rng = random.Random(1000 + repeat)
            indices = [rng.randrange(args.files) if n % 10 else None for n in range(args.requests)]
            paths = [relative(n) if n is not None else 'files/missing.bin' for n in indices]
            expected = [content(n) if n is not None else None for n in indices]
            data = (json.dumps({'id': 0, 'op': 'cat_many', 'paths': paths})+'\n').encode()
            if len(data) > 65536:
                parser.error('generated request exceeds the CLI line limit')
            engines = [('baseline', args.baseline_loom), ('loom', args.loom)]
            if repeat % 2:
                engines.reverse()
            for name, binary in engines:
                elapsed, result = command(root, [binary, 'query', '--batch', '@'+revision,
                                                '--cache-bytes', '0'], data)
                parse_loom_multi(result, expected)
                samples[name].append(elapsed)
            if (repeat+1) % 10 == 0:
                print(f'Checked {repeat+1} paired samples', flush=True)
    operations = {name: summary(values) for name, values in samples.items()}
    report = {'schema_version': 1, 'files': args.files, 'directories': args.directories,
              'requests_per_process': args.requests, 'history_commits': 11,
              'cache_budget_bytes': 0, 'samples_are_paired_by_seed': True,
              'conditions': ['fresh process per sample; same fixture and pinned revision',
                             'warm OS caches; no off-timer query warmup',
                             'engine order alternates; all bytes and slots checked after timing',
                             'native comparison only; no Git ratio',
                             'CPU time, RSS, cold disk and concurrent clients unmeasured'],
              'loom_sha256': hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
              'baseline_loom_sha256': hashlib.sha256(Path(args.baseline_loom).read_bytes()).hexdigest(),
              'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'host': {'platform': platform.platform(), 'cpus': os.cpu_count(),
                       'cpu_model': next((line.split(':', 1)[1].strip()
                                          for line in Path('/proc/cpuinfo').read_text().splitlines()
                                          if line.startswith('model name')), platform.processor())
                       if Path('/proc/cpuinfo').exists() else platform.processor()},
              'operations': operations,
              'baseline_over_loom_p50': operations['baseline']['p50_ms']/operations['loom']['p50_ms'],
              'paired_baseline_over_loom': [a/b for a,b in zip(samples['baseline'], samples['loom'])]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'baseline_over_loom_p50': report['baseline_over_loom_p50']}), flush=True)


if __name__ == '__main__':
    main()
