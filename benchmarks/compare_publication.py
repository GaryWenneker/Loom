#!/usr/bin/env python3
"""Checked full commit workflows, including per-process POSIX resource usage."""
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

from compare_git import command, parse_git_batch, parse_loom_batch, summary


def content(index, size, version=0):
    header = f'file={index:08d};version={version};'.encode()
    return (header + (b'\x00\xffverified\n' * (size//10+1)))[:size]


def measured(root, argv):
    env = {**{k: v for k, v in os.environ.items()
              if not k.startswith('GIT_') and k not in ('LOOM_OBJECT_DIR', 'LOOM_OBJECT_DB')},
           'LOOM_AUTHOR': 'benchmark', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': os.devnull}
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        start = time.perf_counter_ns()
        with subprocess.Popen(argv, cwd=root, env=env, stdout=output, stderr=errors) as child:
            timer = threading.Timer(600, child.kill)
            timer.daemon = True
            timer.start()
            try:
                _, status, usage = os.wait4(child.pid, 0)
                elapsed = (time.perf_counter_ns()-start)/1e6
                child.returncode = os.waitstatus_to_exitcode(status)
            finally:
                timer.cancel()
                timer.join()
            if child.returncode:
                errors.seek(0)
                raise RuntimeError(f'{argv}: {errors.read(2000).decode(errors="replace")}')
        output.seek(0)
        stdout = output.read()
    return {'wall_ms': elapsed, 'user_cpu_ms': usage.ru_utime*1000,
            'system_cpu_ms': usage.ru_stime*1000,
            'cpu_ms': (usage.ru_utime+usage.ru_stime)*1000,
            'max_rss_kib': usage.ru_maxrss/(1024 if platform.system() == 'Darwin' else 1),
            'rusage_inblock': usage.ru_inblock, 'rusage_oublock': usage.ru_oublock}, stdout


def combined(values):
    return {key: (max(v[key] for v in values) if key == 'max_rss_kib'
                  else sum(v[key] for v in values)) for key in values[0]}


def distribution(values):
    return {key.removesuffix("_ms"): value for key, value in summary(values).items()}


def scenario(args, count):
    names = ['baseline', 'loom', 'git']
    phases = ['initial', 'changed_status', 'incremental', 'clean_status']
    samples = {name: {phase: [] for phase in phases} for name in names}
    roots = {name: [] for name in names if name != 'git'}
    footprints = {name: [] for name in names}
    def relative(n):
        leaf = f'{n:08d}.bin'
        bucket = n % args.directories
        if args.layout == 'wide': return f'files/{leaf}'
        if args.layout == 'nested': return f'files/{bucket % 16:02x}/{bucket // 16:02x}/{leaf}'
        return f'files/{bucket:02x}/{leaf}'
    paths = [relative(n) for n in range(count)]
    changed = set(range(0, count, max(1, count//8)))
    changed = set(sorted(changed)[:8])
    for repeat in range(args.repeat):
        order = names[repeat % 3:] + names[:repeat % 3]
        for name in order:
            binary = args.baseline_loom if name == 'baseline' else args.loom
            with tempfile.TemporaryDirectory(prefix='loom-publication-compare-') as temporary:
                root = Path(temporary)
                if name == 'git':
                    command(root, ['git', 'init', '-q'])
                    for key, value in [('user.name', 'Benchmark'), ('user.email', 'bench@example.invalid'),
                                       ('core.autocrlf', 'false'), ('gc.auto', '0'), ('maintenance.auto', 'false')]:
                        command(root, ['git', 'config', key, value])
                else:
                    command(root, [binary, 'init'])
                for n, path in enumerate(paths):
                    target = root/path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(content(n, args.file_bytes))
                recorded_roots = []
                scan_options = (['--scan-workers', str(args.scan_workers)]
                                if name == 'loom' and args.scan_workers is not None else [])
                for phase in ['initial', 'incremental']:
                    if phase == 'incremental':
                        for n in changed:
                            (root/paths[n]).write_bytes(content(n, args.file_bytes, 1))
                        status_command = (['git', 'status', '--porcelain=v1', '--untracked-files=all']
                                          if name == 'git' else [binary, 'status', *scan_options])
                        metrics, status = measured(root, status_command)
                        samples[name]['changed_status'].append(metrics)
                        if name == 'git':
                            actual = {line[3:] for line in status.decode().splitlines()}
                        else:
                            lines = status.decode().splitlines()
                            assert lines[0] == 'On branch main', lines
                            assert all(line.startswith('M ') for line in lines[1:]), lines
                            actual = {line[2:] for line in lines[1:]}
                        assert actual == {paths[n] for n in changed}, 'changed status mismatch'
                    if name == 'git':
                        add, _ = measured(root, ['git', 'add', '--', 'files'])
                        commit, _ = measured(root, ['git', '-c', 'commit.gpgsign=false', 'commit', '-qm', phase])
                        metrics = combined([add, commit])
                        _, revision = command(root, ['git', 'rev-parse', 'HEAD'])
                        revision = revision.decode().strip()
                        _, listing = command(root, ['git', 'ls-tree', '-r', '--name-only', '-z', revision])
                    else:
                        metrics, output = measured(root, [binary, 'commit', '-m', phase, *scan_options])
                        revision = output.decode().strip().split()[-1]
                        _, listing = command(root, [binary, 'files', '@'+revision, '--null'])
                    samples[name][phase].append(metrics)
                    assert listing == b''.join(p.encode()+b'\0' for p in sorted(paths)), 'path listing mismatch'
                    expected = [content(n, args.file_bytes, int(phase == 'incremental' and n in changed))
                                for n in range(count)]
                    if name == 'git':
                        data = ''.join(revision+':'+p+'\n' for p in paths).encode()
                        _, output = command(root, ['git', 'cat-file', '--batch'], data)
                        parse_git_batch(output, expected)
                        command(root, ['git', 'fsck', '--full'])
                    else:
                        data = ''.join(json.dumps({'id': n, 'op': 'cat', 'path': p})+'\n'
                                       for n, p in enumerate(paths)).encode()
                        _, output = command(root, [binary, 'query', '--batch', '@'+revision], data)
                        parse_loom_batch(output, expected)
                        command(root, [binary, 'fsck'])
                        _, stats = command(root, [binary, 'query', '--batch', '@'+revision],
                                           b'{"id":0,"op":"stats"}\n')
                        recorded_roots.append(json.loads(stats)['root'])
                status_command = (['git', 'status', '--porcelain=v1', '--untracked-files=all']
                                  if name == 'git' else [binary, 'status', *scan_options])
                metrics, status = measured(root, status_command)
                samples[name]['clean_status'].append(metrics)
                assert status == (b'' if name == 'git' else b'On branch main\nWorking tree clean\n'), 'clean status mismatch'
                if name != 'git':
                    roots[name].append(recorded_roots)
                metadata = root/('.git' if name == 'git' else '.loom')
                footprints[name].append(sum(p.stat().st_size for p in metadata.rglob('*') if p.is_file()))
        assert roots['baseline'][-1] == roots['loom'][-1], 'canonical native roots differ'
        print(f'Checked {args.layout} {count} files: repeat {repeat+1}/{args.repeat}', flush=True)
    operations = {name: {phase: {metric: distribution([row[metric] for row in values])
                                for metric in values[0]} for phase, values in phases.items()}
                  for name, phases in samples.items()}
    return {'files': count, 'file_bytes': args.file_bytes, 'layout': args.layout,
            'leaf_directories': 1 if args.layout == 'wide' else min(args.directories, count),
            'changed_files': len(changed), 'operations': operations,
            'requested_scan_workers': args.scan_workers,
            'canonical_roots': roots, 'metadata_logical_bytes_after_both_commits': footprints,
            'baseline_over_loom_p50': {p: operations['baseline'][p]['wall_ms']['p50']/operations['loom'][p]['wall_ms']['p50']
                                      for p in phases},
            'git_over_loom_p50': {p: operations['git'][p]['wall_ms']['p50']/operations['loom'][p]['wall_ms']['p50']
                                 for p in phases}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom', required=True, type=lambda p: str(Path(p).resolve()))
    parser.add_argument('--baseline-loom', required=True, type=lambda p: str(Path(p).resolve()))
    parser.add_argument('--files', nargs='+', type=int, default=[1000, 10000])
    parser.add_argument('--repeat', type=int, default=6)
    parser.add_argument('--directories', type=int, default=256)
    parser.add_argument('--layout', choices=['wide', 'sharded', 'nested'], default='sharded')
    parser.add_argument('--file-bytes', type=int, default=256)
    parser.add_argument('--scan-workers', type=int, help='new Loom only: 0=auto, capped at 8/available CPUs')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not hasattr(os, 'wait4'):
        parser.error('per-process resource measurements require POSIX wait4')
    if min(args.files) < 1 or max(args.files) > 1000000 or args.repeat < 1 or args.directories < 1:
        parser.error('positive sizes required; at most one million files')
    if not 32 <= args.file_bytes <= 1024*1024:
        parser.error('file bytes must be 32..1048576')
    if args.scan_workers is not None and args.scan_workers < 0:
        parser.error('scan workers must be nonnegative')
    report = {'schema_version': 1, 'git_version': subprocess.check_output(['git', '--version'], text=True).strip(),
              'host': {'platform': platform.platform(), 'cpus': os.cpu_count(),
                       'cpu_model': next((l.split(':', 1)[1].strip() for l in Path('/proc/cpuinfo').read_text().splitlines()
                                          if l.startswith('model name')), platform.processor()) if Path('/proc/cpuinfo').exists() else platform.processor()},
              'loom_sha256': hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
              'baseline_loom_sha256': hashlib.sha256(Path(args.baseline_loom).read_bytes()).hexdigest(),
              'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'metric_units': {'wall_ms': 'milliseconds', 'user_cpu_ms': 'milliseconds',
                               'system_cpu_ms': 'milliseconds', 'cpu_ms': 'milliseconds',
                               'max_rss_kib': 'KiB', 'rusage_inblock': 'kernel counter units, not bytes',
                               'rusage_oublock': 'kernel counter units, not bytes'},
              'conditions': ['filesystem object backend; fresh repositories per engine/sample; identical bytes and changes',
                             'Loom commit-all vs Git add+commit; both full workflows measured, including process startup',
                             'engine order rotates; fixture creation, full byte verification, listings and fsck outside timing',
                             'initial commit then eight changed files; incremental control rescans same tracked fixture',
                             'changed status checked before incremental commit; clean status after byte verification/fsck',
                             'POSIX wait4 per-process user/system CPU, max RSS and raw kernel block-I/O counters',
                             'Git RSS is max of two sequential commands; CPU/I-O/wall are summed',
                             'metadata bytes are logical file lengths, not allocated disk blocks',
                             'OS caches not flushed; no controlled cold disk, network or multi-agent evidence',
                             'publication durability protocol retained; no power-loss certification'], 'scenarios': []}
    for count in args.files:
        report['scenarios'].append(scenario(args, count))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2)+'\n')
        print(json.dumps({'files': count, 'baseline_over_loom_p50': report['scenarios'][-1]['baseline_over_loom_p50'],
                          'git_over_loom_p50': report['scenarios'][-1]['git_over_loom_p50']}), flush=True)


if __name__ == '__main__':
    main()
