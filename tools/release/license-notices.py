#!/usr/bin/env python3
"""Collect deduplicated license/notice texts from the locked Cargo dependencies."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--target',default='x86_64-unknown-linux-gnu')
args=parser.parse_args();root=Path(__file__).resolve().parents[2]
metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1','--manifest-path',str(root/'Cargo.toml'),'--filter-platform',args.target]))
nodes={node['id']:node for node in metadata['resolve']['nodes']}
reachable=set();pending=[metadata['resolve']['root']]
while pending:
    identity=pending.pop()
    if identity in reachable:continue
    reachable.add(identity);pending.extend(dep['pkg'] for dep in nodes[identity]['deps'])
texts={};packages=[]
for package in sorted(metadata['packages'],key=lambda p:(p['name'],p['version'])):
    if not package['source'] or package['id'] not in reachable:continue
    folder=Path(package['manifest_path']).parent
    candidates=[p for p in folder.iterdir() if p.name.upper().startswith(('LICENSE','LICENCE','COPYING','COPYRIGHT','NOTICE'))]
    if package.get('license_file'):
        file=Path(package['license_file']);file=file if file.is_absolute() else folder/file
        if not file.resolve().is_relative_to(folder.resolve()):raise ValueError('License file outside dependency')
        candidates.append(file)
    found=[]
    for file in sorted(set(candidates)):
        files=[file] if file.is_file() else sorted(p for p in file.rglob('*') if p.is_file()) if file.is_dir() else []
        for item in files:
            if item.is_symlink() or not item.resolve().is_relative_to(folder.resolve()):raise ValueError('Unexpected license path')
            if item.stat().st_size>1024*1024:raise ValueError('Unexpected license size')
            data=item.read_bytes()
            if b'\0' in data:continue
            text=data.decode('utf-8').replace('\r\n','\n');identity=hashlib.sha256(text.encode()).hexdigest()[:16]
            texts[identity]=text;found.append(identity)
    if not found and (package['name'],package['version'],package['license'])==('Inflector','0.11.4','BSD-2-Clause'):
        # This upstream crate declares BSD-2-Clause but omits a separate
        # license file. Include its declared authors and the SPDX reference
        # text, without inventing an upstream copyright year or notice.
        text='Inflector 0.11.4 attribution from Cargo metadata: '+', '.join(package['authors'])+'\nRepository: '+str(package['repository'])+'\nThe crate omits a separate license file. The following is the SPDX BSD-2-Clause reference text, not a reconstructed upstream notice.\n'+(root/'licenses/SPDX-BSD-2-Clause.txt').read_text()
        identity=hashlib.sha256(text.encode()).hexdigest()[:16];texts[identity]=text;found.append(identity)
    if not found:raise ValueError('No bundled license text found for '+package['name']+' '+package['version'])
    packages.append(package['name']+' '+package['version']+' | '+str(package['license'])+' | texts '+', '.join(sorted(set(found))))
lines=['RIRRd beta — third-party dependency license texts','Generated from the locked Cargo metadata. Selected Linux target dependencies, including build dependencies, are included.','Project code: MIT. Dependency licenses and notices remain their own.','',*packages]
for identity,text in sorted(texts.items()):lines+=['','='*72,'License/notice text '+identity,'='*72,text]
args.output.write_text('\n'.join(lines)+'\n')
print(f'Collected notices for {len(packages)} locked packages, {len(texts)} distinct texts')
