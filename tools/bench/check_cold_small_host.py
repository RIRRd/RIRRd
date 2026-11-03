#!/usr/bin/env python3
"""Recheck the 16 MiB profile after evicting only its own clean redb file pages."""
import argparse,ctypes,hashlib,json,mmap,os,subprocess,time
from pathlib import Path
from urllib.request import Request,urlopen

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--run',type=Path,required=True)
parser.add_argument('--binary',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
assert not args.output.exists() and os.geteuid()==0
base=json.loads((args.run/'results.json').read_text());assert base.get('completed_at')
assert hashlib.sha256(args.binary.read_bytes()).hexdigest()==base['binary_sha256']['small16']
file=args.run/'small16-3.redb';cfg=args.run/'small16-3.toml'
assert file.is_file() and cfg.is_file()

def residency():
 with file.open('rb') as source:
  mapping=mmap.mmap(source.fileno(),0,access=mmap.ACCESS_COPY)
  try:
   pages=(len(mapping)+mmap.PAGESIZE-1)//mmap.PAGESIZE
   vector=(ctypes.c_ubyte*pages)()
   libc=ctypes.CDLL(None,use_errno=True)
   libc.mincore.argtypes=[ctypes.c_void_p,ctypes.c_size_t,ctypes.POINTER(ctypes.c_ubyte)]
   libc.mincore.restype=ctypes.c_int
   assert libc.mincore(ctypes.addressof(ctypes.c_char.from_buffer(mapping)),len(mapping),vector)==0
   return {'pages':pages,'resident':sum(bool(value&1) for value in vector)}
  finally:mapping.close()

before=residency()
with file.open('rb') as source:
 os.fsync(source.fileno());os.posix_fadvise(source.fileno(),0,0,os.POSIX_FADV_DONTNEED)
after=residency()
assert after['resident']<=1,'per-file cache eviction was incomplete; do not label this cold-start'
unit='rirrd-small-cold-'+args.run.name[-6:]
result={'scope':'one-CPU 16 MiB-cache common-query check after verified per-file page-cache eviction; not a whole-VM minimum','candidate_commit':base['candidate_commit'],'binary_sha256':base['binary_sha256']['small16'],'fixture_sha256':base['fixture_sha256'],'page_residency_before':before,'page_residency_after_eviction':after,'cpu_quota':'100%','server_cpus':'0','server_memory_max_bytes':128*1024**2,'profiles':[]}
try:
 subprocess.run(['systemd-run','--collect','--unit',unit,'--property=CPUAffinity=0','--property=CPUQuota=100%','--property=MemoryMax=128M','--property=TasksMax=64','--property=MemoryAccounting=yes',str(args.binary),'--config',str(cfg),'server'],check=True,capture_output=True)
 deadline=time.monotonic()+30
 while True:
  try:
   req=Request('http://127.0.0.1:9280/graphql/',data=json.dumps({'query':'{ databaseStatus { source } }'}).encode(),headers={'Content-Type':'application/json'})
   with urlopen(req,timeout=2) as response:assert json.load(response)['data']['databaseStatus']==[{'source':'PERF'}]
   break
  except (OSError,AssertionError,KeyError):
   assert time.monotonic()<deadline;time.sleep(.05)
 load=Path(__file__).resolve().parents[2]/'target/release/examples/registry_load'
 for mode,port in [('whois',9243),('graphql',9280)]:
  output=subprocess.check_output(['taskset','-c','16-19',str(load),'--endpoint',f'127.0.0.1:{port}','--mode',mode,'--seconds','10','--warmup','3','--clients','4','--keys','50000'],text=True,timeout=45)
  profile=json.loads(output);assert not profile['errors'];result['profiles'].append(profile)
 values=dict(line.split('=',1) for line in subprocess.check_output(['systemctl','show',unit,'-p','MemoryCurrent','-p','MemoryPeak','-p','MainPID','-p','ControlGroup'],text=True).splitlines())
 result['cgroup_current_bytes']=int(values['MemoryCurrent']);result['cgroup_peak_bytes']=int(values['MemoryPeak'])
 result['memory_events']=dict((name,int(value)) for name,value in (line.split() for line in (Path('/sys/fs/cgroup')/values['ControlGroup'].lstrip('/')/'memory.events').read_text().splitlines()))
 assert result['memory_events']['oom_kill']==0
 for line in Path('/proc',values['MainPID'],'smaps_rollup').read_text().splitlines():
  if line.startswith('Pss:'):result['process_pss_bytes']=int(line.split()[1])*1024
 result['completed_at']=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
 args.output.write_text(json.dumps(result,indent=2)+'\n')
 print(json.dumps(result))
finally:subprocess.run(['systemctl','stop',unit],check=False,capture_output=True)
