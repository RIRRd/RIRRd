#!/usr/bin/env python3
"""Compare RIRRd cache profiles on one CPU with bounded systemd cgroups.

Runtime data stays outside Git. Requires the original 100k synthetic PERF fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from urllib.request import Request,urlopen

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--state',type=Path,required=True)
parser.add_argument('--fixture',type=Path,required=True)
parser.add_argument('--before',type=Path,required=True)
parser.add_argument('--before-commit',default='92b3f9f42b09549b7006e2068da69ce05ec0b4e7')
parser.add_argument('--before-cache-mib',type=int,choices=range(1,65537),metavar='MiB')
parser.add_argument('--candidate',type=Path,required=True)
parser.add_argument('--commit',required=True)
parser.add_argument('--seconds',type=int,default=10)
parser.add_argument('--whois-port',type=int,default=9243)
parser.add_argument('--http-port',type=int,default=9280)
args=parser.parse_args()
assert os.geteuid()==0 and {0,16,17,18,19}.issubset(os.sched_getaffinity(0))
assert 1<=args.seconds<=120
assert 1<=args.whois_port<=65535 and 1<=args.http_port<=65535 and args.whois_port!=args.http_port
# Never run a benchmark against an already serving registry by accident.
for port in [args.whois_port,args.http_port]:
 with socket.socket() as probe:
  probe.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
  probe.bind(('127.0.0.1',port))
fixture=args.fixture.resolve();state=args.state.resolve();state.mkdir(mode=0o700,exist_ok=True)
assert hashlib.sha256(fixture.read_bytes()).hexdigest()=='9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2'
run=Path(tempfile.mkdtemp(prefix='measured-',dir=state))
root=Path(__file__).resolve().parents[2]
load=root/'target/release/examples/registry_load'
variants={'before':(args.before.resolve(),args.before_cache_mib),'default64':(args.candidate.resolve(),64),'small16':(args.candidate.resolve(),16)}
results={'scope':'100k synthetic common AS-key queries; constrained RIRRd profiles, not full registry qualification','candidate_commit':args.commit,'before_commit':args.before_commit,'cache_mib':{name:cache for name,(_,cache) in variants.items()},'ports':{'whois':args.whois_port,'http':args.http_port},'fixture_sha256':hashlib.sha256(fixture.read_bytes()).hexdigest(),'binary_sha256':{name:hashlib.sha256(path.read_bytes()).hexdigest() for name,(path,_) in variants.items()},'driver_sha256':hashlib.sha256(load.read_bytes()).hexdigest(),'server_cpus':'0','cpu_quota':'100%','driver_cpus':'16-19','import_memory_max_bytes':256*1024**2,'server_memory_max_bytes':128*1024**2,'clients':4,'imports':[],'profiles':[],'memory_samples':[]}
log_number=0
unit_counter=0
unit=None

def save(): (run/'results.json').write_text(json.dumps(results,indent=2)+'\n')
def command(argv,timeout=300,check=True):
 global log_number
 response=subprocess.run(list(map(str,argv)),capture_output=True,text=True,timeout=timeout)
 log_number+=1;(run/f'command-{log_number}.log').write_text(response.stdout+response.stderr)
 if check and response.returncode:raise RuntimeError(f'command failed; see {run}/command-{log_number}.log')
 return response

def stop():
 if unit: subprocess.run(['systemctl','stop',unit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)

def new_unit(kind):
 global unit_counter,unit
 stop();unit_counter+=1;unit=f'rirrd-small-{run.name[-6:]}-{kind}-{unit_counter}'
 return ['systemd-run','--collect','--unit',unit,'--property=CPUAffinity=0','--property=CPUQuota=100%','--property=TasksMax=64','--property=MemoryAccounting=yes']

def config(name,rep):
 cache=variants[name][1]
 path=run/f'{name}-{rep}.toml'
 text=f'database = "{run}/{name}-{rep}.redb"\nsources = ["PERF"]\nlisten = "127.0.0.1:{args.whois_port}"\nhttp_listen = "127.0.0.1:{args.http_port}"\nmax_connections = 8\nio_timeout_seconds = 10\nmax_snapshot_bytes = 16777216\n'
 if cache is not None:text+=f'database_cache_mib = {cache}\nworker_threads = 1\nmax_blocking_threads = 4\n'
 path.write_text(text);return path

def show():
 values=dict(line.split('=',1) for line in command(['systemctl','show',unit,'-p','MemoryCurrent','-p','MemoryPeak','-p','Result','-p','ExecMainStatus','-p','MainPID']).stdout.splitlines() if '=' in line)
 for key in ['MemoryCurrent','MemoryPeak','ExecMainStatus','MainPID']:
  values[key]=int(values[key]) if values[key].isdigit() else None
 return values

helper=run/'import_helper.py'
helper.write_text('''import json,resource,subprocess,sys,time
from pathlib import Path
start=time.perf_counter()
r=subprocess.run(sys.argv[2:],capture_output=True,text=True)
u=resource.getrusage(resource.RUSAGE_CHILDREN)
value={'seconds':time.perf_counter()-start,'cli_peak_rss_bytes':u.ru_maxrss*1024,'cli_cpu_seconds':u.ru_utime+u.ru_stime,'exit_code':r.returncode,'report':None}
if r.returncode==0:value['report']=json.loads(r.stdout)
Path(sys.argv[1]).write_text(json.dumps(value))
raise SystemExit(r.returncode)
''')

def ready():
 deadline=time.monotonic()+30
 query='{ rpslObjects(rpslPk:["AS4200000000"],sources:["PERF"]) { rpslPk source objectClass } }'
 while time.monotonic()<deadline:
  try:
   req=Request(f'http://127.0.0.1:{args.http_port}/graphql/',data=json.dumps({'query':query}).encode(),headers={'Content-Type':'application/json'})
   with urlopen(req,timeout=2) as response:value=json.load(response)
   assert value['data']['rpslObjects']==[{'rpslPk':'AS4200000000','source':'PERF','objectClass':'aut-num'}]
   return
  except (OSError,AssertionError,KeyError,ValueError):time.sleep(.05)
 raise RuntimeError('candidate did not become query ready')

def pss(pid):
 for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
  if line.startswith('Pss:'):return int(line.split()[1])*1024
 raise RuntimeError('PSS unavailable')

try:
 names=list(variants)
 for rep in range(1,4):
  order=names[rep-1:]+names[:rep-1]
  for name in order:
   binary,_=variants[name];cfg=config(name,rep);report=run/f'{name}-{rep}-import.json'
   response=command(new_unit('import')+['--property=MemoryMax=256M','--property=Type=oneshot','--property=RemainAfterExit=yes',sys.executable,helper,report,binary,'--config',cfg,'import','snapshot','--source','PERF','--file',fixture],check=False)
   data=json.loads(report.read_text()) if report.exists() else {'exit_code':None}
   limits=show();data.update({'variant':name,'repetition':rep,'cgroup_peak_bytes':limits['MemoryPeak'],'unit_result':limits['Result']})
   assert response.returncode==0 and data['exit_code']==0 and data['report']['objects']==100000,(name,'bounded import failed',data)
   results['imports'].append(data);save();print('import',name,rep,round(data['seconds'],3),'s',round(data['cli_peak_rss_bytes']/1048576,1),'MiB CLI peak',flush=True)
   stop()
   verified=json.loads(command(['taskset','-c','0',binary,'--config',cfg,'database','verify']).stdout)
   assert verified['objects']==100000,verified
   data['verified_objects']=verified['objects'];save()
 for rep in range(1,4):
  order=names[rep-1:]+names[:rep-1]
  for name in order:
   binary,_=variants[name];cfg=run/f'{name}-3.toml'
   command(new_unit('query')+['--property=MemoryMax=128M',binary,'--config',cfg,'server'])
   ready()
   assert Path('/proc',str(show()['MainPID']),'exe').resolve()==binary,'query server identity mismatch'
   for mode in ['whois','graphql']:
    before=resource.getrusage(resource.RUSAGE_CHILDREN);start=time.monotonic()
    port=args.whois_port if mode=='whois' else args.http_port
    value=json.loads(command(['taskset','-c','16-19',load,'--endpoint',f'127.0.0.1:{port}','--mode',mode,'--seconds',args.seconds,'--warmup','3','--clients','4','--keys','50000'],timeout=args.seconds+30).stdout)
    after=resource.getrusage(resource.RUSAGE_CHILDREN)
    value.update({'variant':name,'repetition':rep,'driver_cpu_fraction_of_four_cores':((after.ru_utime+after.ru_stime)-(before.ru_utime+before.ru_stime))/(4*(time.monotonic()-start))})
    assert not value['errors'] and value['successful']==value['attempted']
    results['profiles'].append(value);save();print('query',name,mode,rep,round(value['queries_per_second']),'q/s',flush=True)
   time.sleep(1)
   samples=[]
   for _ in range(3):
    value=show();pid=int(value['MainPID']);samples.append({'process_pss_bytes':pss(pid),'cgroup_current_bytes':value['MemoryCurrent'],'cgroup_peak_bytes':value['MemoryPeak']});time.sleep(.2)
   results['memory_samples'].append({'variant':name,'repetition':rep,'samples':samples});save();stop()
 results['disk']={name:{'allocated_bytes':(run/f'{name}-3.redb').stat().st_blocks*512,'apparent_bytes':(run/f'{name}-3.redb').stat().st_size} for name in variants}
 results['completed_at']=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime());save();print('COMPLETE',run/'results.json',flush=True)
finally:stop()
