#!/usr/bin/env python3
"""Build the project showcase; no community directory or queue is bundled here."""
import argparse
import hashlib
import html
import json
from pathlib import Path
import shutil
import subprocess
import markdown

ROOT=Path(__file__).resolve().parent
CORE=ROOT.parents[1]

def document(title,body):
    return f'''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{html.escape(title)} · RIRRd beta</title><link rel="icon" href="/brand/rirrd.svg" type="image/svg+xml"><link rel="stylesheet" href="/project.css"><script src="/project.js" defer></script></head><body><a class="skip" href="#main">Skip to content</a><header class="shell masthead"><a class="brand" href="/"><img src="/brand/rirrd.svg" width="40" height="40" alt=""><strong>RIRRd</strong></a><button class="menu-toggle secondary" type="button" aria-expanded="false" aria-controls="site-navigation">Menu</button><nav id="site-navigation" aria-label="Main"><a href="/#software">Software</a><a href="/#benchmarks">Benchmarks</a><a href="/docs/getting-started/">Documentation</a><a href="/testing/">Testing</a><a href="/docs/beta/">Beta status</a><a class="mobile-only" href="https://irrd.tools/">Try irrd.tools</a></nav><a class="button secondary" href="https://irrd.tools/">Try irrd.tools ↗</a></header><main id="main" class="shell document">{body}</main><footer class="shell"><div>RIRRd · MIT-licensed Rust software</div><div><a href="https://irrd.tools/">Community network</a><a href="https://github.com/Peravix/Rirrd/issues">Report an issue</a></div><p>Beta · AI-assisted development<br>Full IRRd parity remains in development.</p></footer></body></html>'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=ROOT/'public')
    parser.add_argument('--allow-dirty',action='store_true')
    args=parser.parse_args()
    if subprocess.check_output(['git','-C',str(CORE),'status','--porcelain']) and not args.allow_dirty:
        parser.error('Commit source changes before publishing')
    revision=subprocess.check_output(['git','-C',str(CORE),'rev-parse','HEAD'],text=True).strip()
    out=args.output;out.mkdir(parents=True,exist_ok=True);files=[]
    def copy(source,target):
        destination=out/target;destination.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,destination);files.append(target)
    for name in ['index.html','project.css','project.js']:copy(ROOT/'assets'/name,name)
    copy(CORE/'site/brand/rirrd.svg','brand/rirrd.svg')
    copy(CORE/'web/dashboard/charts.js','charts.js')
    copy(CORE/'site/coming-soon/public/comparison.json','comparison.json')
    copy(CORE/'LICENSE','license.txt')
    copy(ROOT/'assets/testing.html','testing/index.html')
    copy(ROOT/'assets/testing.js','testing.js')
    copy(ROOT/'verification.json','testing/report.json')
    release=json.loads((ROOT/'release.json').read_text())
    if release['repository_public']:
        p=out/'index.html';p.write_text(p.read_text().replace('Source publication pending','Open-source project'))
    for folder,title,source in [('getting-started','Self-host RIRRd beta',CORE/'docs/operations/GETTING_STARTED.md'),('api','API and query commands',CORE/'docs/operations/API_COMMANDS.md'),('routers','BIRD and FRR integration',CORE/'docs/operations/PUBLIC_ROUTER_QUERIES.md'),('rpki','Embedded RPKI validation',CORE/'docs/operations/INTEGRATED_RPKI.md'),('beta','Beta status and known gaps',CORE/'docs/BETA_STATUS.md')]:
        destination=out/'docs'/folder/'index.html';destination.parent.mkdir(parents=True,exist_ok=True)
        body=markdown.markdown(source.read_text(),extensions=['fenced_code','tables'])
        body=body.replace('href="ROUTER_INTEGRATION.md"','href="/docs/routers/"')
        destination.write_text(document(title,body));files.append('docs/'+folder+'/index.html')
    (out/'build.json').write_text(json.dumps({'source_revision':revision,'product':'RIRRd project','release':release,'files':{name:hashlib.sha256((out/name).read_bytes()).hexdigest() for name in files}},indent=2)+'\n')
    print('Built RIRRd project site from',revision,'at',out)

if __name__=='__main__':main()
