#!/usr/bin/env python3
"""Check publication filenames/blobs and optionally stage a secret-scan snapshot."""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--repo',type=Path,default=Path(__file__).resolve().parents[1])
parser.add_argument('--snapshot',type=Path,help='new destination for an exact candidate-tree scan')
parser.add_argument('--include-untracked',action='store_true')
args=parser.parse_args()
root=args.repo.resolve()
command=['git','-C',str(root),'ls-files','--cached','-z']
if args.include_untracked:command+=['--others','--exclude-standard']
paths={Path(name.decode()) for name in subprocess.check_output(command).split(b'\0') if name}
forbidden_suffixes={'.redb','.sqlite','.sqlite3','.db','.key','.p12','.pfx','.log','.pyc'}
forbidden_parts={'.ssh','.aws','.gnupg','__pycache__','target'}
def problem(path):
    name=path.name
    if any(ord(c)<32 for c in str(path)):return 'control character in filename'
    if name=='.env' or (name.startswith('.env.') and name!='.env.example'):return 'local environment file'
    if name.startswith(('id_rsa','id_ed25519')):return 'SSH identity file'
    if path.suffix.lower() in forbidden_suffixes:return 'runtime, credential or database file'
    if set(path.parts)&forbidden_parts:return 'private/cache directory'
    if path.parts[:2] in [('fuzz','corpus'),('fuzz','artifacts')]:return 'fuzz runtime data'
    return None
failures=[]
for rel in sorted(paths):
    file=root/rel
    reason=problem(rel)
    if reason:failures.append((str(rel),reason));continue
    if file.is_symlink():failures.append((str(rel),'symlink requires explicit publication review'));continue
    if file.is_file() and file.stat().st_size>1024*1024:failures.append((str(rel),'over 1 MiB; review artifacts and fixture licensing'))
# Reachable historical names and sizes, across all fetched branches and tags.
objects=subprocess.check_output(['git','-C',str(root),'rev-list','--objects','--all'],text=True).splitlines()
named={line.split(' ',1)[0]:line.split(' ',1)[1] for line in objects if ' ' in line}
blob_data=subprocess.check_output(['git','-C',str(root),'cat-file','--batch-check=%(objectname) %(objecttype) %(objectsize)'],input='\n'.join(named)+'\n',text=True)
history_blobs=0
for line in blob_data.splitlines():
    oid,kind,size=line.split()
    if kind!='blob':continue
    history_blobs+=1
    rel=Path(named[oid]);reason=problem(rel)
    if reason:failures.append((str(rel),'history: '+reason))
    if int(size)>1024*1024:failures.append((str(rel),'history: blob over 1 MiB'))
if failures:
    for name,reason in sorted(set(failures)):print(reason+': '+name,file=sys.stderr)
    raise SystemExit(1)
if args.snapshot:
    args.snapshot.mkdir(mode=0o700)
    for rel in paths:
        file=root/rel
        if file.is_file():
            target=args.snapshot/rel;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(file,target)
print(f'Publication file audit passed: {len(paths)} candidate paths, {history_blobs} reachable historical blobs; no forbidden runtime/credential paths or large blobs')
