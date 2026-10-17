#!/usr/bin/env python3
"""Checked incremental add+commit workflows over explicit files/subtrees."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tempfile

from compare_git import command, parse_git_batch, parse_loom_batch
from compare_publication import content, measured, combined, distribution


def scenario(args, count):
    engines = ['baseline', 'loom', 'git']
    paths = [f'files/{n % args.directories:04x}/{n:08d}.bin' for n in range(count)]
    changed = set(range(min(args.changed_paths, args.directories, count)))
    expanded = [p for n, p in enumerate(paths) if n % args.directories in changed]
    directories = [f'files/{n:04x}' for n in sorted(changed)]
    expected_listing = b''.join(p.encode()+b'\0' for p in sorted(paths))
    expected_path_hash = hashlib.sha256(expected_listing).hexdigest()
    samples = {e: {p: {'add': [], 'commit': [], 'complete': []} for p in ['files', 'directories']}
               for e in engines}
    bootstrap = {}
    roots = {e: [] for e in engines if e != 'git'}
    revisions = {e: [] for e in engines}
    with tempfile.TemporaryDirectory(prefix='loom-pathset-compare-') as temporary:
        workspaces = {e: Path(temporary)/e for e in engines}
        for engine, root in workspaces.items():
            root.mkdir()
            binary = args.baseline_loom if engine == 'baseline' else args.loom
            if engine == 'git':
                command(root, ['git', 'init', '-q'])
                for key, value in [('user.name','Benchmark'),('user.email','bench@example.invalid'),
                                   ('core.autocrlf','false'),('gc.auto','0'),('maintenance.auto','false')]:
                    command(root, ['git','config',key,value])
            else:
                command(root, [args.baseline_loom,'init'])
            for n, name in enumerate(paths):
                target = root/name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content(n, args.file_bytes))
            if engine == 'git':
                command(root, ['git','add','--','files'])
                command(root, ['git','-c','commit.gpgsign=false','commit','-qm','base'])
                bootstrap[engine], _ = measured(root, ['git','add','--', paths[0]])
            else:
                command(root, [args.baseline_loom,'commit','-m','base'])
                bootstrap[engine], _ = measured(root, [binary,'add',paths[0]])
                command(root, [binary,'fsck'])
        step = 0
        for phase in ['files', 'directories']:
            for repeat in range(args.repeat):
                step += 1
                order = engines[repeat % 3:] + engines[:repeat % 3]
                for engine in order:
                    root = workspaces[engine]
                    binary = args.baseline_loom if engine == 'baseline' else args.loom
                    for n in changed:
                        (root/paths[n]).write_bytes(content(n, args.file_bytes, step))
                    selected = ([paths[n] for n in sorted(changed)] if phase == 'files'
                                else (expanded if engine == 'baseline' and not args.baseline_directories else directories))
                    worker_request = (args.baseline_scan_workers if engine == 'baseline'
                                      else args.scan_workers)
                    worker_args = (['--scan-workers',str(worker_request)]
                                   if engine != 'git' and worker_request is not None else [])
                    add_args = (['git','add','--',*selected] if engine == 'git'
                                else [binary,'add',*worker_args,*selected])
                    commit_args = (['git','-c','commit.gpgsign=false','commit','-qm',str(step)]
                                   if engine == 'git' else [binary,'commit',*worker_args,'-m',str(step)])
                    add, _ = measured(root, add_args)
                    commit, output = measured(root, commit_args)
                    samples[engine][phase]['add'].append(add)
                    samples[engine][phase]['commit'].append(commit)
                    samples[engine][phase]['complete'].append(combined([add,commit]))
                    if engine == 'git':
                        _, output = command(root,['git','rev-parse','HEAD'])
                        revision = output.decode().strip()
                        _, listing = command(root,['git','ls-tree','-r','--name-only','-z',revision])
                        data = ''.join(revision+':'+paths[n]+'\n' for n in sorted(changed)).encode()
                        _, output = command(root,['git','cat-file','--batch'],data)
                        parse_git_batch(output,[content(n,args.file_bytes,step) for n in sorted(changed)])
                    else:
                        revision = output.decode().strip().split()[-1]
                        _, listing = command(root,[binary,'files','@'+revision,'--null'])
                        data = ''.join(json.dumps({'id':i,'op':'cat','path':paths[n]})+'\n'
                                       for i,n in enumerate(sorted(changed))).encode()
                        _, output = command(root,[binary,'query','--batch','@'+revision],data)
                        parse_loom_batch(output,[content(n,args.file_bytes,step) for n in sorted(changed)])
                        _, output = command(root,[binary,'query','--batch','@'+revision],b'{"id":0,"op":"stats"}\n')
                        roots[engine].append(json.loads(output)['root'])
                    assert hashlib.sha256(listing).hexdigest() == expected_path_hash, 'full listing mismatch'
                    revisions[engine].append(revision)
                assert roots['baseline'][-1] == roots['loom'][-1], 'native root mismatch'
                print(f'Checked {count} files, {phase}, repeat {repeat+1}/{args.repeat}',flush=True)
        footprints = {}
        # Full final payload verification and complete history/closure fsck are
        # outside timings; each earlier sample checks all paths and changed bytes.
        expected = [content(n,args.file_bytes,step if n in changed else 0) for n in range(count)]
        for engine, root in workspaces.items():
            revision = revisions[engine][-1]
            binary = args.baseline_loom if engine == 'baseline' else args.loom
            if engine == 'git':
                data = ''.join(revision+':'+p+'\n' for p in paths).encode()
                _, output = command(root,['git','cat-file','--batch'],data)
                parse_git_batch(output,expected)
                command(root,['git','fsck','--full'])
            else:
                data = ''.join(json.dumps({'id':n,'op':'cat','path':p})+'\n' for n,p in enumerate(paths)).encode()
                _, output = command(root,[binary,'query','--batch','@'+revision],data)
                parse_loom_batch(output,expected)
                command(root,[binary,'fsck'])
            footprints[engine] = sum(p.stat().st_size for p in (root/('.git' if engine=='git' else '.loom')).rglob('*') if p.is_file())
    operations = {e:{p:{op:{m:distribution([v[m] for v in values]) for m in values[0]}
                       for op,values in ops.items()} for p,ops in phases.items()} for e,phases in samples.items()}
    return {'files':count,'file_bytes':args.file_bytes,'directories':min(count,args.directories),
            'changed_paths':len(changed),'selected_directory_candidates':len(expanded),
            'loom_scan_workers':args.scan_workers,
            'baseline_scan_workers':args.baseline_scan_workers,
            'baseline_directory_arguments':'same-scopes' if args.baseline_directories else 'expanded-files',
            'cold_first_unchanged_add':bootstrap,'operations':operations,'canonical_roots':roots,
            'metadata_logical_bytes':footprints,
            'baseline_over_loom_p50':{p:operations['baseline'][p]['complete']['wall_ms']['p50']/operations['loom'][p]['complete']['wall_ms']['p50'] for p in ['files','directories']},
            'git_over_loom_p50':{p:operations['git'][p]['complete']['wall_ms']['p50']/operations['loom'][p]['complete']['wall_ms']['p50'] for p in ['files','directories']}}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loom',required=True,type=lambda p:str(Path(p).resolve()))
    parser.add_argument('--baseline-loom',required=True,type=lambda p:str(Path(p).resolve()))
    parser.add_argument('--files',nargs='+',type=int,default=[1000,10000,100000])
    parser.add_argument('--repeat',type=int,default=6)
    parser.add_argument('--directories',type=int,default=256)
    parser.add_argument('--file-bytes',type=int,default=256)
    parser.add_argument('--scan-workers',type=int,
                        help='new Loom add and commit: 0=auto, requests are capped by the engine')
    parser.add_argument('--baseline-scan-workers',type=int,
                        help='baseline add and commit worker request; requires baseline CLI support')
    parser.add_argument('--baseline-directories',action='store_true',
                        help='use identical directory scopes for a baseline with recursive add support')
    parser.add_argument('--changed-paths',type=int,default=8,
                        help='files/selected directories modified per sample')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if min(args.files)<1 or max(args.files)>1000000 or args.repeat<1 or args.directories<1 or args.changed_paths<1 or not 32<=args.file_bytes<=1048576 or any(v is not None and v<0 for v in [args.scan_workers,args.baseline_scan_workers]):
        parser.error('positive valid sizes/repeats required')
    report={'schema_version':2,'host':{'platform':platform.platform(),'cpus':__import__('os').cpu_count()},
            'git_version':subprocess.check_output(['git','--version'],text=True).strip(),
            'loom_scan_workers':args.scan_workers,
            'baseline_scan_workers':args.baseline_scan_workers,
            'baseline_directory_arguments':'same-scopes' if args.baseline_directories else 'expanded-files',
            'loom_sha256':hashlib.sha256(Path(args.loom).read_bytes()).hexdigest(),
            'baseline_sha256':hashlib.sha256(Path(args.baseline_loom).read_bytes()).hexdigest(),
            'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            'resource_helper_sha256':hashlib.sha256(Path(__file__).with_name('compare_publication.py').read_bytes()).hexdigest(),
            'conditions':['complete add plus commit, both startups timed, existing filesystem-backed repositories',
                          'initial full-repository commit/setup outside timing; one workspace per engine/size reused with identical history depth',
                          'first unchanged add/bootstrap recorded separately; incremental timings have current native path indexes',
                          f'{args.changed_paths} files/selected directories requested per sample, files then directory scopes; engine order rotates per repeat',
                          f'new Loom add+commit worker request: {args.scan_workers}; omitted means up to four workers',
                          f'baseline add+commit worker request: {args.baseline_scan_workers}; omitted uses that binary default',
                          ('both native engines receive identical directory scopes' if args.baseline_directories else
                           'legacy adapted control: baseline receives an expanded file list; argument counts differ and this is not a same-input directory comparison'),
                          'every sample checks full path listing, selected payload bytes/framing and native root equality',
                          'full final payload verification and full history/closure fsck after all samples',
                          'warm uncontrolled caches; no cold-disk, network, crash or concurrent-agent evidence',
                          'CPU milliseconds, raw wait4 RSS KiB with possible launcher floor; kernel I/O counters are platform units, not bytes',
                          'no isolated RSS reduction claim; logical metadata bytes are not allocated disk blocks'],'scenarios':[]}
    for count in args.files:
        result=scenario(args,count)
        report['scenarios'].append(result)
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({'files':count,'baseline_directory_arguments':result['baseline_directory_arguments'],
                          'baseline_scan_workers':args.baseline_scan_workers,'loom_scan_workers':args.scan_workers,
                          'baseline_over_loom_p50':result['baseline_over_loom_p50'],'git_over_loom_p50':result['git_over_loom_p50']}),flush=True)


if __name__=='__main__': main()
