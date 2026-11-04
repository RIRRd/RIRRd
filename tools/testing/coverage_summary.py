#!/usr/bin/env python3
"""Summarize only project src files; fail if required coverage is absent or too low."""
import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
from harness import ROOT

parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('coverage',type=Path);parser.add_argument('--out',type=Path,required=True);parser.add_argument('--minimum',type=float,default=0)
args=parser.parse_args();data=json.loads(args.coverage.read_text());files=[]
for group in data['data']:
    for item in group['files']:
        path=Path(item['filename']).resolve()
        if path.is_relative_to(ROOT/'src'):
            files.append({'file':str(path.relative_to(ROOT)),'lines':item['summary']['lines']})
if not files:raise SystemExit('No project source coverage recorded')
covered=sum(f['lines']['covered']for f in files);count=sum(f['lines']['count']for f in files);percentage=100*covered/count
result={'schema_version':1,'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'source_dirty':bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),'origin':'github_actions' if os.environ.get('GITHUB_ACTIONS')=='true' else 'local','recorded_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'scope':'LLVM line coverage of src/*.rs from Rust tests and isolated protocol contracts; test modules inside src are included; dependency, test harness and fixture files are excluded','lines':{'covered':covered,'count':count,'percent':percentage},'minimum':args.minimum,'files':files}
args.out.write_text(json.dumps(result,indent=2)+'\n')
if os.environ.get('GITHUB_STEP_SUMMARY'):
    with open(os.environ['GITHUB_STEP_SUMMARY'],'a')as f:f.write(f"## Source coverage\n\n{percentage:.2f}% ({covered}/{count} source lines). Enforced floor: {args.minimum:.2f}%. Test modules inside src are included; external fixtures/harnesses/dependencies are excluded.\n")
print(f'Project src line coverage: {percentage:.2f}% ({covered}/{count}); floor {args.minimum:.2f}%')
if percentage<args.minimum:raise SystemExit('Project line coverage fell below its enforced floor')
