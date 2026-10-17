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


def parse_loom_multi(output, expected, rounds=1):
    offset = 0
    group_size = len(expected)//rounds
    for round_id in range(rounds):
        end = output.index(b'\n', offset); header = json.loads(output[offset:end]); offset = end + 1
        assert header['kind'] == 'blobs' and header['ok'] and header['id'] == round_id, header
        wanted_group = expected[round_id*group_size:(round_id+1)*group_size]
        assert len(header['entries']) == len(wanted_group), header
        for entry, wanted in zip(header['entries'], wanted_group):
            if wanted is None:
                assert entry is None, entry
            else:
                assert entry['size'] == len(wanted), entry
                assert output[offset:offset+len(wanted)] == wanted
                offset += len(wanted)
        assert offset-end-1 == header['size'] and output[offset:offset+1] == b'\n'
        offset += 1
    assert offset == len(output), 'unconsumed Loom grouped output'


def scenario(args, count):
    timings = {'loom_cat_cli': [], 'git_cat_cli': [], 'loom_files_cli': [],
               'git_files_cli': [], 'loom_status_cli': [], 'git_status_cli': [],
               'git_batch': [], 'loom_batch': []}
    if args.baseline_loom:
        for operation in ['cat_cli','files_cli','status_cli','batch']:
            timings['baseline_'+operation] = []
    if args.multi: timings['loom_multi'] = []
    if args.baseline_multi: timings['baseline_multi'] = []
    if args.read_pack:
        timings['loom_pack_batch'] = []
        timings['loom_pack_multi'] = []
    if args.rounds > 1 and args.multi:
        timings['git_first_group'] = []
        timings['loom_first_group'] = []
        if args.baseline_multi: timings['baseline_first_group'] = []
    initializer = args.baseline_loom or args.loom
    def relative(index):
        name = f'{index:08d}.bin'
        bucket = index % args.directories
        if args.layout == 'wide': return f'files/{name}'
        if args.layout == 'nested': return f'files/{bucket % 16:02x}/{bucket // 16:02x}/{name}'
        return f'files/{bucket:02x}/{name}'
    with tempfile.TemporaryDirectory(prefix='loom-git-compare-') as temporary:
        root = Path(temporary)
        command(root, ['git', 'init', '-q'])
        command(root, ['git', 'config', 'user.name', 'Benchmark'])
        command(root, ['git', 'config', 'user.email', 'benchmark@example.invalid'])
        command(root, ['git', 'config', 'core.autocrlf', 'false'])
        command(root, ['git', 'config', 'gc.auto', '0'])
        (root/'.git/info/exclude').write_text('.loom/\n')
        command(root, [initializer, 'init'])
        (root/'files').mkdir()
        for n in range(count):
            path = root/relative(n)
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(content(n))
        _, committed = command(root, [initializer, 'commit', '-m', 'same tracked fixture'])
        revision = committed.decode().strip().split()[-1]
        command(root, ['git', 'add', '--', 'files'])
        command(root, ['git', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'same tracked fixture'])
        # Build an explicit history with changes away from the queried files.
        for n in range(args.history):
            (root/'noise').write_text(f'change {n}\n')
            command(root, [initializer, 'add', 'noise'])
            _, committed = command(root, [initializer, 'commit', '-m', f'noise {n}'])
            revision = committed.decode().strip().split()[-1]
            command(root, ['git', 'add', '--', 'noise'])
            command(root, ['git', '-c', 'commit.gpgsign=false', 'commit', '-qm', f'noise {n}'])
        if args.git_layout == 'packed':
            command(root, ['git', 'gc', '--quiet'])
            command(root, ['git', 'commit-graph', 'write', '--reachable', '--changed-paths'])
        _, git_revision = command(root, ['git','rev-parse','HEAD'])
        git_revision = git_revision.decode().strip()
        preparation = None
        if args.read_pack:
            elapsed, _ = command(root, [args.loom, 'read-pack', '@'+revision, '.loom/reads.pack'])
            preparation = {'build_ms':elapsed, 'pack_bytes':(root/'.loom/reads.pack').stat().st_size,
                           'query_timing_excludes_build':True, 'live_objects_retained':True}
        focus = relative(count//2)
        def query(binary):
            return [binary, 'query', '--batch', '@'+revision,
                    '--cache-bytes', str(args.cache_bytes)]
        for repeat in range(args.repeat):
            # Alternate order to reduce systematic warmup/host-load bias.
            pairs = [
                ('cat_cli', [args.loom, 'cat', '@'+revision, focus], ['git', 'show', git_revision+':'+focus]),
                ('files_cli', [args.loom, 'files', '@'+revision, focus, '--null'],
                 ['git', 'ls-tree', '-r', '--name-only', '-z', git_revision, '--', focus]),
                ('status_cli', [args.loom, 'status'], ['git', 'status', '--porcelain=v1']),
            ]
            for name, loom_command, git_command in pairs:
                engines = ['loom','git'] + (['baseline'] if args.baseline_loom else [])
                engines = engines[repeat % len(engines):] + engines[:repeat % len(engines)]
                for engine in engines:
                    selected = git_command if engine == 'git' else loom_command
                    if engine == 'baseline': selected = [args.baseline_loom, *loom_command[1:]]
                    elapsed, output = command(root, selected)
                    if name == 'cat_cli': assert output == content(count//2)
                    elif name == 'files_cli': assert output == focus.encode()+b'\0'
                    elif engine == 'git': assert not output, output
                    else: assert b'Working tree clean' in output, output
                    timings[engine+'_'+name].append(elapsed)
            rng = random.Random(1000 + repeat)
            indices = [rng.randrange(count) if n % 10 else None for n in range(args.requests)]
            paths = [relative(n) if n is not None else 'files/missing.bin' for n in indices]
            expected = [content(n) if n is not None else None for n in indices]
            # Rotate one fixed working set across groups in the same process.
            # Git and both Loom versions receive the identical expanded sequence.
            if args.rounds > 1:
                paths = [path for n in range(args.rounds) for path in paths[n % len(paths):]+paths[:n % len(paths)]]
                expected = [value for n in range(args.rounds) for value in expected[n % len(expected):]+expected[:n % len(expected)]]
            git_input = ''.join(git_revision+':'+path+'\n' for path in paths).encode()
            loom_input = ''.join(json.dumps({'id':i,'op':'cat','path':path})+'\n' for i,path in enumerate(paths)).encode()
            modes = [('git_batch', ['git','cat-file','--batch'], git_input, parse_git_batch)]
            if args.batch:
                modes.append(('loom_batch', query(args.loom), loom_input, parse_loom_batch))
                if args.baseline_loom:
                    modes.append(('baseline_batch', query(args.baseline_loom), loom_input, parse_loom_batch))
            if args.multi:
                multi_input = ''.join(json.dumps({'id':n,'op':'cat_many','paths':paths[n*args.requests:(n+1)*args.requests]})+'\n' for n in range(args.rounds)).encode()
                verify_multi = lambda output, expected: parse_loom_multi(output, expected, args.rounds)
                modes.append(('loom_multi',query(args.loom),multi_input,verify_multi))
                if args.baseline_multi:
                    modes.append(('baseline_multi',query(args.baseline_loom),multi_input,verify_multi))
                if args.read_pack:
                    base_command = query(args.loom) + ['--read-pack','.loom/reads.pack']
                    modes.append(('loom_pack_batch',base_command,loom_input,parse_loom_batch))
                    modes.append(('loom_pack_multi',base_command,multi_input,verify_multi))
                if args.rounds > 1:
                    first_paths, first_expected = paths[:args.requests], expected[:args.requests]
                    first_git_input = ''.join(git_revision+':'+path+'\n' for path in first_paths).encode()
                    first_multi_input = (json.dumps({'id':0,'op':'cat_many','paths':first_paths})+'\n').encode()
                    modes.append(('git_first_group',['git','cat-file','--batch'],first_git_input,
                                  lambda output, _: parse_git_batch(output, first_expected)))
                    modes.append(('loom_first_group',query(args.loom),first_multi_input,
                                  lambda output, _: parse_loom_multi(output, first_expected)))
                    if args.baseline_multi:
                        modes.append(('baseline_first_group',query(args.baseline_loom),first_multi_input,
                                      lambda output, _: parse_loom_multi(output, first_expected)))
            modes = modes[repeat % len(modes):] + modes[:repeat % len(modes)]
            for name, argv, data, verify in modes:
                elapsed, output = command(root, argv, data)
                verify(output, expected)
                timings[name].append(elapsed)
        operations = {name:summary(samples) for name,samples in timings.items() if samples}
        ratios = {name:operations['git_'+name]['p50_ms']/operations['loom_'+name]['p50_ms']
                  for name in ['cat_cli','files_cli','status_cli','batch']
                  if 'loom_'+name in operations}
        if args.multi: ratios['multi'] = operations['git_batch']['p50_ms']/operations['loom_multi']['p50_ms']
        if 'loom_first_group' in operations:
            ratios['first_group'] = operations['git_first_group']['p50_ms']/operations['loom_first_group']['p50_ms']
        if args.read_pack:
            for mode in ['pack_batch', 'pack_multi']:
                ratios[mode] = operations['git_batch']['p50_ms']/operations['loom_'+mode]['p50_ms']
        baseline_ratios = {name:operations['baseline_'+name]['p50_ms']/operations['loom_'+name]['p50_ms']
                           for name in ['cat_cli','files_cli','status_cli','batch']
                           if 'baseline_'+name in operations and 'loom_'+name in operations}
        if args.multi and args.baseline_loom:
            baseline_group = 'baseline_multi' if args.baseline_multi else 'baseline_batch'
            baseline_ratios['multi'] = operations[baseline_group]['p50_ms']/operations['loom_multi']['p50_ms']
            if args.read_pack:
                baseline_ratios['pack_multi'] = operations[baseline_group]['p50_ms']/operations['loom_pack_multi']['p50_ms']
                baseline_ratios['pack_batch'] = operations['baseline_batch']['p50_ms']/operations['loom_pack_batch']['p50_ms']
        if 'baseline_first_group' in operations:
            baseline_ratios['first_group'] = operations['baseline_first_group']['p50_ms']/operations['loom_first_group']['p50_ms']
        return {'tracked_files':count, 'history_commits':args.history+1,
                'requests_per_batch':args.requests, 'git_layout':args.git_layout,
                'rounds_per_process':args.rounds, 'requests_per_process':args.requests*args.rounds,
                'fixture_layout':args.layout+'; small binary files; 10% batch misses',
                'directories':args.directories if args.layout!='wide' else 1,
                'cache_budget_bytes':args.cache_bytes,
                'repository_logical_bytes':{name:sum(p.stat().st_size for p in (root/name).rglob('*') if p.is_file()) for name in ['.git','.loom']},
                'operations':operations, 'git_over_loom_p50':ratios,
                'baseline_over_loom_p50':baseline_ratios,
                'baseline_grouped_mode':'cat_many' if args.baseline_multi else 'individual cat lines',
                'read_pack_preparation':preparation,
                'unpacked_over_packed_p50':{mode:operations['loom_'+mode]['p50_ms']/operations['loom_pack_'+mode]['p50_ms']
                                            for mode in ['batch','multi']} if args.read_pack else {},
                'target_100x_met':{key:value >= 100 for key,value in ratios.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom', required=True, type=lambda x:str(Path(x).resolve()))
    parser.add_argument('--baseline-loom', type=lambda x:str(Path(x).resolve()))
    parser.add_argument('--multi', action='store_true', help='Also measure native cat_many on the same path sequence')
    parser.add_argument('--baseline-multi', action='store_true', help='Baseline binary also supports cat_many')
    parser.add_argument('--read-pack', action='store_true', help='Prepare and compare an explicit native read pack; retains loose objects')
    parser.add_argument('--directories', type=int, default=256)
    parser.add_argument('--files', nargs='+', type=int, default=[1000,10000])
    parser.add_argument('--history', type=int, default=10)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--requests', type=int, default=1000)
    parser.add_argument('--rounds', type=int, default=1, help='Repeat one rotating working set within each persistent process')
    parser.add_argument('--cache-bytes', type=int, default=64*1024*1024,
                        help='Snapshot cache budget for both Loom query binaries; zero disables it')
    parser.add_argument('--layout', choices=['wide','sharded','nested'], default='wide',
                        help='one wide directory or files spread over 256 directories')
    parser.add_argument('--git-layout', choices=['loose','packed'], default='packed')
    parser.add_argument('--batch', action='store_true', help='Measure new Loom persistent query mode too')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.directories < 1 or min(args.files) < 1 or args.repeat < 1 or args.requests < 1 or args.history < 0:
        parser.error('positive sizes/repetitions/requests and nonnegative history are required')
    if not 1 <= args.rounds <= 100 or args.requests*args.rounds > 100000:
        parser.error('--rounds must be 1..100, with at most 100000 total logical requests per process')
    if not 0 <= args.cache_bytes <= 256*1024*1024:
        parser.error('--cache-bytes must be between zero and 256 MiB')
    if args.multi and (not args.batch or args.requests > 4096):
        parser.error('--multi requires --batch and at most 4096 logical requests')
    if (args.baseline_multi and not args.baseline_loom) or ((args.read_pack or args.baseline_multi) and not args.multi):
        parser.error('--read-pack requires --multi; --baseline-multi requires --multi and --baseline-loom')
    version = subprocess.check_output(['git','--version'],text=True).strip()
    report = {'schema_version':4, 'git_version':version,
              'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'baseline_loom_sha256':hashlib.sha256(Path(args.baseline_loom).read_bytes()).hexdigest() if args.baseline_loom else None,
              'loom_sha256':hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
              'host':{'platform':platform.platform(),'machine':platform.machine(),'cpus':os.cpu_count(),
                      'cpu_model':next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),platform.processor()) if Path('/proc/cpuinfo').exists() else platform.processor(),
                      'filesystem':'overlay (container)' if Path('/.dockerenv').exists() else 'not detected'},
              'conditions':['same tracked bytes; both queries pin immutable revision IDs', 'output verified byte for byte',
                            'process startup included; OS caches warm, not a cold-disk experiment',
                            'rounds reuse and rotate the same working set; all engines keep one process; timings cover the entire sequence including its first group',
                            'cat_many groups the same paths into one native request; existing line mode is reported separately',
                            'Git cat-file batch compared against Loom batch, not repeated Git launches',
                            'packed Git uses gc and a changed-path commit graph',
                            'explicit native read-pack build time/size reported separately; live objects retained; fresh session opens pack index each sample',
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
