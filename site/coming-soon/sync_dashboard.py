#!/usr/bin/env python3
"""Copy the bundled RIRRd UI and generate either a synthetic preview or a live read-only dashboard."""
import argparse
import hashlib
import ipaddress
import json
from pathlib import Path
import shutil
import subprocess

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('core',type=Path)
parser.add_argument('--live',action='store_true',help='public read-only deployment; requires installed local query gateway')
parser.add_argument('--allow-dirty',action='store_true',help='local preview only; rerun from a clean commit before publishing')
args=parser.parse_args()
core=args.core.resolve()
status=subprocess.check_output(['git','-C',str(core),'status','--porcelain'],text=True)
if status and not args.allow_dirty:parser.error('core checkout must be clean to publish a traceable dashboard')
revision=subprocess.check_output(['git','-C',str(core),'rev-parse','HEAD'],text=True).strip()
if status:revision+='-working-tree'
root=Path(__file__).resolve().parent
out=root/'public/dashboard';out.mkdir(parents=True,exist_ok=True)
for name in ['index.html','style.css','app.js','network.json']:
 shutil.copyfile(core/'web/dashboard'/name,out/name)
meta={'mode':'preview','version':'0.1.0','source_revision':revision,'node':{'name':'RIRRd dashboard preview','location':None,'sponsor':None},'sources':['EXAMPLE'],'whois_listen':None,'max_connections':8,'database_cache_mib':16}
if args.live:
 meta.update(mode='live',public_service=True,node={'name':'RIRRd public test registry','location':None,'sponsor':None},sources=[r['source'] for r in json.loads((core/'tools/live-service/sources.json').read_text())['sources'] if r['state']=='enabled'],whois_listen='rirrd.co.uk:43',max_connections=128,database_cache_mib=64)
if args.live:
 profile=json.loads((core/'tools/live-service/node.json').read_text())
 meta['node']={**profile,'sponsor':profile['provider']}
 for source,name in [('index.html','index.html'),('style.css','node.css'),('app.js','node.js')]:
  shutil.copyfile(core/'site/coming-soon/legacy-node'/source,root/'public'/name)
(out/'meta.json').write_text(json.dumps(meta,indent=2)+'\n')
fixture=(core/'tools/router-integration/fixture.rpsl').read_text()
records=[]
for block in fixture.strip().split('\n\n'):
 fields={}
 for line in block.splitlines():
  key,value=line.split(':',1);fields.setdefault(key,[]).append(value.strip())
 assert fields['source']==['EXAMPLE']
 records.append((fields,block+'\n'))
assert len(records)==12,'review changes to the public synthetic fixture'
objects={};origins={};sets={};members={}
for fields,block in records:
 cls=next(iter(fields))
 if cls=='aut-num':
  assert fields[cls][0] in ['AS64496','AS64497']
 if cls in ['route','route6']:
  net=ipaddress.ip_network(fields[cls][0]);assert net.subnet_of(ipaddress.ip_network('2001:db8::/32') if net.version==6 else ipaddress.ip_network('192.0.2.0/24') if str(net).startswith('192.') else ipaddress.ip_network('198.51.100.0/24'))
  origins.setdefault(fields['origin'][0],{}).setdefault(str(net.version),[]).append(str(net))
 if cls in ['aut-num','route','route6','as-set','route-set']:
  assert 'auth' not in fields and 'certif' not in fields
  key=fields[cls][0].upper();objects[key]=objects.get(key,'')+block+'\n'
 if cls=='as-set':members[fields[cls][0]]=fields.get('members',[])
for name in members:
 todo=[name];seen=set();asns=set()
 while todo:
  item=todo.pop()
  if item in seen:continue
  seen.add(item)
  for line in members.get(item,[]):
   for member in map(str.strip,line.split(',')):
    if member in members:todo.append(member)
    else:asns.add(member)
 sets[name]={family:sorted({p for asn in asns for p in origins.get(asn,{}).get(family,[])}) for family in ['4','6']}
sample={'scope':'original synthetic documentation fixture only','fixture_sha256':hashlib.sha256(fixture.encode()).hexdigest(),'objects':objects,'origins':origins,'sets':sets,'sources':[{'source':'EXAMPLE','authoritative':False,'serialNewestMirror':None,'nrtm4ClientVersion':None,'lastUpdate':None,'rpslDataUpdated':None}]}
(out/'sample-results.json').write_text(json.dumps(sample,indent=2)+'\n')
shutil.copyfile(core/'LICENSE',root/'public/license.txt')
# Render the same maintained guide used by repository readers.
import markdown
from html import escape
md=(core/'docs/operations/ROUTER_INTEGRATION.md').read_text()
for old,new in [('../../examples/routers/bird.conf','bird.conf'),('../../examples/routers/frr.conf','frr.conf'),('../../tools/router-integration/README.md','https://github.com/Peravix/Rirrd/tree/codex/linux-parity/tools/router-integration')]:md=md.replace(old,new)
body=markdown.markdown(md,extensions=['fenced_code','tables'])
guide=root/'public/guides/routers';guide.mkdir(parents=True,exist_ok=True)
html='''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>RIRRd · BIRD and FRR integration</title><link rel="stylesheet" href="style.css"></head><body><a class="skip" href="#guide">Skip to guide</a><header><a href="/">RIRRd</a><nav><a href="/dashboard/">Live dashboard</a><a href="/#performance">Measurements</a></nav></header><main id="guide"><p class="notice">Development guide · MIT-licensed · Built with AI assistance · AI contributions welcome; review and testing required.</p>'''+body+'''</main><footer><a href="/license.txt">MIT license</a><span>Repository currently private · Experimental read-only test registry</span><a href="/">Project site ↗</a></footer></body></html>'''
(guide/'index.html').write_text(html)
for name in ['bird.conf','frr.conf']:shutil.copyfile(core/'examples/routers'/name,guide/name)
(guide/'style.css').write_text('''*{box-sizing:border-box}body{margin:0;background:#101c22;color:#edf3ef;font:16px/1.75 system-ui,sans-serif}a{color:#a7efcc;text-underline-offset:3px}a:focus-visible{outline:3px solid #a7efcc;outline-offset:5px}header,footer,main{width:min(900px,100% - 40px);margin:auto}header,footer{display:flex;justify-content:space-between;gap:20px;flex-wrap:wrap;padding-block:25px;border-bottom:1px solid #30434b}header>a{font-size:25px;font-weight:700;text-decoration:none}nav{display:flex;gap:20px;font-size:13px}main{padding-block:30px 60px}h1{font-size:clamp(30px,5vw,44px);line-height:1.2;letter-spacing:-.04em}h2{font-size:25px;line-height:1.3;margin-top:50px}p,li{color:#c1d0d2;font-size:15px}pre{overflow:auto;max-width:100%;background:#1b3036;border:1px solid #38534f;border-radius:7px;padding:20px;font:12px/1.8 ui-monospace,Consolas,monospace;color:#bef0d6}code{font:0.85em ui-monospace,Consolas,monospace;overflow-wrap:anywhere}pre code{overflow-wrap:normal}footer{border-top:1px solid #30434b;border-bottom:0;font-size:11px;color:#a7b9bf}.notice{padding:15px;background:#1c3530;border:1px solid #3b5e4e;border-radius:5px;font-size:12px}.skip{position:absolute;top:-100px;left:20px}.skip:focus{top:10px}''')
api=root/'public/guides/api';api.mkdir(parents=True,exist_ok=True)
api_body=markdown.markdown((core/'docs/operations/API_COMMANDS.md').read_text(),extensions=['fenced_code','tables'])
api_html=html[:html.index('<h1>')]+api_body+html[html.index('</main>'):]
api_html=api_html.replace('RIRRd · BIRD and FRR integration','RIRRd · IRR API and commands')
(api/'index.html').write_text(api_html)
shutil.copyfile(guide/'style.css',api/'style.css')
print('Synced dashboard',meta['mode'],'from',revision)
