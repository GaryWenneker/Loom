#!/usr/bin/env python3
"""Compare equivalent tracked-file queries, with checked output and raw samples."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import tempfile
import time


def command(root, argv, data=None, timeout=600):
    start = time.perf_counter_ns()
    result = subprocess.run(argv, cwd=root, input=data, capture_output=True, timeout=timeout,
                            env={**{k:v for k,v in os.environ.items() if not k.startswith('GIT_')
                                    and k not in ('LOOM_OBJECT_DIR','LOOM_OBJECT_DB')}, 'LOOM_AUTHOR': 'benchmark',
                                 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': os.devnull})
    elapsed = (time.perf_counter_ns() - start) / 1e6
    if result.returncode:
        raise RuntimeError(f'{argv}: {result.stderr.decode(errors="replace")[:2000]}')
    return elapsed, result.stdout


def summary(values):
    ordered = sorted(values)
    def percentile(q):
        position = (len(ordered) - 1) * q
        lo = int(position); hi = min(lo + 1, len(ordered) - 1)
        return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)
    return {'samples_ms': values, 'p50_ms': statistics.median(values),
            'p95_ms': percentile(.95), 'p99_ms': percentile(.99)}


def content(index):
    return f'file={index:08d}:'.encode() + bytes([index % 256]) + b' verified\x00content\n'


def parse_git_batch(output, expected):
    offset = 0
    for wanted in expected:
        end = output.index(b'\n', offset); header = output[offset:end]; offset = end + 1
        if wanted is None:
            assert header.endswith(b' missing'), header
        else:
            fields = header.split(); assert fields[-2] == b'blob', header
            size = int(fields[-1]); assert output[offset:offset+size] == wanted
            offset += size; assert output[offset:offset+1] == b'\n'; offset += 1
    assert offset == len(output), 'unconsumed Git batch output'


def parse_loom_batch(output, expected):
    offset = 0
    for request_id, wanted in enumerate(expected):
        end = output.index(b'\n', offset); header = json.loads(output[offset:end]); offset = end + 1
        assert header['id'] == request_id, header
        if wanted is None:
            assert header['kind'] == 'missing', header
        else:
            assert header['kind'] == 'blob' and header['ok'], header
            size = header['size']; assert output[offset:offset+size] == wanted
            offset += size; assert output[offset:offset+1] == b'\n'; offset += 1
    assert offset == len(output), 'unconsumed Loom batch output'


def scenario(args, count):
    timings = {'loom_cat_cli': [], 'git_cat_cli': [], 'loom_files_cli': [],
               'git_files_cli': [], 'loom_status_cli': [], 'git_status_cli': [],
               'git_batch': [], 'loom_batch': []}
    def relative(index):
        name = f'{index:08d}.bin'
        return f'files/{name}' if args.layout == 'wide' else f'files/{index % 256:02x}/{name}'
    with tempfile.TemporaryDirectory(prefix='loom-git-compare-') as temporary:
        root = Path(temporary)
        command(root, ['git', 'init', '-q'])
        command(root, ['git', 'config', 'user.name', 'Benchmark'])
        command(root, ['git', 'config', 'user.email', 'benchmark@example.invalid'])
        command(root, ['git', 'config', 'core.autocrlf', 'false'])
        command(root, ['git', 'config', 'gc.auto', '0'])
        (root/'.git/info/exclude').write_text('.loom/\n')
        command(root, [args.loom, 'init'])
        (root/'files').mkdir()
        for n in range(count):
            path = root/relative(n)
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(content(n))
        _, committed = command(root, [args.loom, 'commit', '-m', 'same tracked fixture'])
        revision = committed.decode().strip().split()[-1]
        command(root, ['git', 'add', '--', 'files'])
        command(root, ['git', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'same tracked fixture'])
        # Build an explicit history with changes away from the queried files.
        for n in range(args.history):
            (root/'noise').write_text(f'change {n}\n')
            command(root, [args.loom, 'add', 'noise'])
            _, committed = command(root, [args.loom, 'commit', '-m', f'noise {n}'])
            revision = committed.decode().strip().split()[-1]
            command(root, ['git', 'add', '--', 'noise'])
            command(root, ['git', '-c', 'commit.gpgsign=false', 'commit', '-qm', f'noise {n}'])
        if args.git_layout == 'packed':
            command(root, ['git', 'gc', '--quiet'])
            command(root, ['git', 'commit-graph', 'write', '--reachable', '--changed-paths'])
        _, git_revision = command(root, ['git','rev-parse','HEAD'])
        git_revision = git_revision.decode().strip()
        focus = relative(count//2)
        for repeat in range(args.repeat):
            # Alternate order to reduce systematic warmup/host-load bias.
            pairs = [
                ('cat_cli', [args.loom, 'cat', '@'+revision, focus], ['git', 'show', git_revision+':'+focus]),
                ('files_cli', [args.loom, 'files', '@'+revision, focus, '--null'],
                 ['git', 'ls-tree', '-r', '--name-only', '-z', git_revision, '--', focus]),
                ('status_cli', [args.loom, 'status'], ['git', 'status', '--porcelain=v1']),
            ]
            for name, loom_command, git_command in pairs:
                for engine in (['loom','git'] if repeat % 2 == 0 else ['git','loom']):
                    elapsed, output = command(root, loom_command if engine == 'loom' else git_command)
                    if name == 'cat_cli': assert output == content(count//2)
                    elif name == 'files_cli': assert output == focus.encode()+b'\0'
                    elif engine == 'git': assert not output, output
                    else: assert b'Working tree clean' in output, output
                    timings[engine+'_'+name].append(elapsed)
            rng = random.Random(1000 + repeat)
            indices = [rng.randrange(count) if n % 10 else None for n in range(args.requests)]
            paths = [relative(n) if n is not None else 'files/missing.bin' for n in indices]
            expected = [content(n) if n is not None else None for n in indices]
            git_input = ''.join(git_revision+':'+path+'\n' for path in paths).encode()
            loom_input = ''.join(json.dumps({'id':i,'op':'cat','path':path})+'\n' for i,path in enumerate(paths)).encode()
            for engine in (['loom','git'] if repeat % 2 == 0 else ['git','loom']):
                if engine == 'loom' and not args.batch: continue
                elapsed, output = command(root, [args.loom, 'query', '--batch', '@'+revision] if engine == 'loom'
                                          else ['git', 'cat-file', '--batch'], loom_input if engine == 'loom' else git_input)
                (parse_loom_batch if engine == 'loom' else parse_git_batch)(output, expected)
                timings[engine+'_batch'].append(elapsed)
        operations = {name:summary(samples) for name,samples in timings.items() if samples}
        ratios = {name:operations['git_'+name]['p50_ms']/operations['loom_'+name]['p50_ms']
                  for name in ['cat_cli','files_cli','status_cli','batch']
                  if 'loom_'+name in operations}
        return {'tracked_files':count, 'history_commits':args.history+1,
                'requests_per_batch':args.requests, 'git_layout':args.git_layout,
                'fixture_layout':args.layout+'; small binary files; 10% batch misses',
                'cache_budget_bytes':64*1024*1024,
                'repository_logical_bytes':{name:sum(p.stat().st_size for p in (root/name).rglob('*') if p.is_file()) for name in ['.git','.loom']},
                'operations':operations, 'git_over_loom_p50':ratios,
                'target_100x_met':{key:value >= 100 for key,value in ratios.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom', required=True, type=lambda x:str(Path(x).resolve()))
    parser.add_argument('--files', nargs='+', type=int, default=[1000,10000])
    parser.add_argument('--history', type=int, default=10)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--requests', type=int, default=1000)
    parser.add_argument('--layout', choices=['wide','sharded'], default='wide',
                        help='one wide directory or files spread over 256 directories')
    parser.add_argument('--git-layout', choices=['loose','packed'], default='packed')
    parser.add_argument('--batch', action='store_true', help='Measure new Loom persistent query mode too')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if min(args.files) < 1 or args.repeat < 1 or args.requests < 1 or args.history < 0:
        parser.error('positive sizes/repetitions/requests and nonnegative history are required')
    version = subprocess.check_output(['git','--version'],text=True).strip()
    report = {'schema_version':1, 'git_version':version,
              'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'loom_sha256':hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
              'host':{'platform':platform.platform(),'machine':platform.machine(),'cpus':os.cpu_count(),
                      'cpu_model':next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),platform.processor()) if Path('/proc/cpuinfo').exists() else platform.processor(),
                      'filesystem':'overlay (container)' if Path('/.dockerenv').exists() else 'not detected'},
              'conditions':['same tracked bytes; both queries pin immutable revision IDs', 'output verified byte for byte',
                            'process startup included; OS caches warm, not a cold-disk experiment',
                            'Git cat-file batch compared against Loom batch, not repeated Git launches',
                            'packed Git uses gc and a changed-path commit graph',
                            'peak RSS, CPU time, cold disk and multi-client throughput are not measured',
                            'no performance threshold is treated as proven for unmeasured operations'],
              'scenarios':[]}
    for count in args.files:
        print(f'Compare {count} tracked files...',flush=True)
        report['scenarios'].append(scenario(args,count))
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report['scenarios'][-1]['git_over_loom_p50']),flush=True)


if __name__ == '__main__': main()
