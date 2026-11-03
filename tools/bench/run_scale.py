#!/usr/bin/env python3
"""Root-only VM benchmark controller for the disposable rirrd-scale Docker project.

Uses a generated 100k PERF fixture, never downloads registry data. Timed reads
are validated by examples/registry_load.rs. Runtime files remain outside Git.
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
parser.add_argument('--candidate',type=Path,required=True)
parser.add_argument('--candidate-commit',required=True)
parser.add_argument('--before',type=Path)
parser.add_argument('--seconds',type=int,default=20)
parser.add_argument('--resume',type=Path,help='reuse three completed imports after a failed query harness run; reruns ALL query profiles')
parser.add_argument('--resume-reason',help='required with --resume; public explanation for rerunning every query profile')
args=parser.parse_args()
if bool(args.resume) != bool(args.resume_reason):
    parser.error('--resume and --resume-reason must be supplied together')
if args.resume_reason and len(args.resume_reason)>512:
    parser.error('--resume-reason is limited to 512 characters')
assert os.geteuid()==0 and set(range(24)).issubset(os.sched_getaffinity(0)), 'requires root and CPUs 0..23'
assert 1<=args.seconds<=120
root=Path(__file__).resolve().parents[2]
state=args.state.resolve()
os.environ['RIRRD_BENCH_STATE']=str(state)
fixture=state/'synthetic-100k.rpsl'
meta=json.loads((state/'fixture.json').read_text())
assert meta['objects']==100000 and hashlib.sha256(fixture.read_bytes()).hexdigest()==meta['sha256']
run=args.resume.resolve() if args.resume else Path(tempfile.mkdtemp(prefix='measured-',dir=state))
assert run.parent==state, 'resume must be an immediate child of state'
compose=['docker','compose','-p','rirrd-scale','-f',str(root/'tools/oracle/compose.yaml'),'-f',str(state/'compose.yaml')]
load=root/'target/release/examples/registry_load'
candidate=args.candidate.resolve()
unit='rirrd-scale-candidate'
start_counter=0
log_counter=max([int(p.stem.split('-')[1]) for p in run.glob('command-*.log')],default=0)

def command(argv,timeout=1800):
    global log_counter
    result=subprocess.run(list(map(str,argv)),capture_output=True,text=True,timeout=timeout)
    log_counter+=1
    (run/f'command-{log_counter}.log').write_text(result.stdout+result.stderr)
    if result.returncode:
        raise RuntimeError(f'command failed: {argv[0:3]}; see {run}/command-{log_counter}.log')
    return result.stdout.strip()

def save():
    (run/'results.json').write_text(json.dumps(results,indent=2)+'\n')

helper="""import json,resource,subprocess,sys,time
start=time.perf_counter()
r=subprocess.run(sys.argv[1:],capture_output=True,text=True)
u=resource.getrusage(resource.RUSAGE_CHILDREN)
print(json.dumps({'seconds':time.perf_counter()-start,'cpu_seconds':u.ru_utime+u.ru_stime,'peak_cli_rss_bytes':u.ru_maxrss*1024,'exit_code':r.returncode,'stdout':r.stdout,'stderr':r.stderr}))
"""

def write_config(index):
    config=run/f'candidate-{index}.toml'
    config.write_text(f'database = "{run}/import-{index}.redb"\nsources = ["PERF"]\nlisten = "127.0.0.1:9043"\nhttp_listen = "127.0.0.1:9080"\nmax_connections = 32\nio_timeout_seconds = 10\nmax_snapshot_bytes = 134217728\n')
    return config

def whois(query):
    with socket.create_connection(('127.0.0.1',8043),timeout=2) as conn:
        conn.sendall(query.encode()+b'\n')
        data=bytearray()
        while block:=conn.recv(8192):
            data.extend(block)
            if len(data)>65536: raise ValueError('oversized readiness response')
        return bytes(data)

def ready(port):
    deadline=time.monotonic()+90
    query='{ rpslObjects(rpslPk:["AS4200000000"],sources:["PERF"]) { rpslPk source objectClass } }'
    while time.monotonic()<deadline:
        try:
            request=Request(f'http://127.0.0.1:{port}/graphql/',data=json.dumps({'query':query}).encode(),headers={'Content-Type':'application/json'})
            with urlopen(request,timeout=2) as response:
                value=json.load(response)
            assert value['data']['rpslObjects']==[{'rpslPk':'AS4200000000','source':'PERF','objectClass':'aut-num'}]
            return
        except (OSError,ValueError,AssertionError,KeyError):
            time.sleep(.05)
    raise RuntimeError('HTTP query readiness deadline exceeded')

def stop_candidate():
    subprocess.run(['systemctl','stop',unit],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)

def start_candidate(binary,config):
    global unit,start_counter
    stop_candidate()
    start_counter+=1
    unit=f'rirrd-scale-candidate-{start_counter}'
    start=time.perf_counter()
    command(['systemd-run','--collect','--unit',unit,'--property=CPUAffinity=0-15','--property=MemoryAccounting=yes',binary,'--config',config,'server'])
    ready(9080)
    pid=int(command(['systemctl','show',unit,'-p','MainPID','--value']))
    assert pid>0 and Path(f'/proc/{pid}/exe').resolve()==Path(binary).resolve(), 'candidate process identity mismatch'
    return time.perf_counter()-start

def process_pss(pid):
    try:
        for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
            if line.startswith('Pss:'): return int(line.split()[1])*1024
    except FileNotFoundError:
        return 0
    raise RuntimeError('PSS unavailable')

def memory():
    reference=[]
    candidate_memory=[]
    for _ in range(3):
        pids=set()
        for service in ['reference','postgres','redis']:
            cid=command(compose+['ps','-q',service])
            pids.update(int(x) for x in command(['docker','top',cid,'-eo','pid']).splitlines() if x.strip().isdigit())
        reference.append(sum(process_pss(pid) for pid in pids))
        pid=int(command(['systemctl','show',unit,'-p','MainPID','--value']))
        candidate_memory.append(process_pss(pid))
        time.sleep(1)
    return {'scope':'summed process PSS; IRRd application/supervision plus PostgreSQL and Redis; resident mmap pages included, unmapped filesystem cache excluded',
            'reference_samples':reference,'candidate_samples':candidate_memory,
            'reference':statistics.median(reference),'candidate':statistics.median(candidate_memory)}

def profile(system,mode,connections,seconds,rep,variant='current',phase='workload'):
    port=8043 if mode=='whois' else 8000
    if system=='candidate': port=9043 if mode=='whois' else 9080
    argv=['taskset','-c','16-23',str(load),'--endpoint',f'127.0.0.1:{port}','--mode',mode,'--connections',connections,'--seconds',str(seconds),'--warmup','3','--clients','8','--keys','50000']
    before=resource.getrusage(resource.RUSAGE_CHILDREN)
    start=time.perf_counter()
    data=json.loads(command(argv,timeout=seconds+30))
    wall=time.perf_counter()-start
    after=resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu=(after.ru_utime+after.ru_stime)-(before.ru_utime+before.ru_stime)
    data.update({'system':system,'repetition':rep,'variant':variant,'phase':phase,'driver_cpu_seconds':cpu,'driver_cpu_fraction_of_eight_cores':cpu/(wall*8)})
    assert not data['errors'] and data['successful']==data['attempted']
    results['profiles'].append(data)
    save()
    print(system,variant,mode,connections,rep,round(data['queries_per_second']), 'q/s',flush=True)

results={'kind':'synthetic 100k common-subset comparison','candidate_commit':args.candidate_commit,
         'candidate_binary_sha256':hashlib.sha256(candidate.read_bytes()).hexdigest(),
         'reference_commit':'8e8080dab744a348b7d0e6efcb53f403301dd075',
         'fixture':meta,'server_cpus':'0-15','client_cpus':'16-23','reference_whois_workers':8,'reference_http_workers':8,
         'candidate_connection_cap':32,'query_clients':8,'imports':[],'profiles':[],
         'load_client_sha256':hashlib.sha256(load.read_bytes()).hexdigest(),
         'controller_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
         'started_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),
         'host':{'kernel':os.uname().release,'architecture':os.uname().machine,
                 'cpu_topology':command(['lscpu','-J']),
                 'os_release':Path('/etc/os-release').read_text(),
                 'mem_total_kib':int(Path('/proc/meminfo').read_text().splitlines()[0].split()[1])}}
if args.resume:
    previous=json.loads((run/'results.json').read_text())
    assert not previous.get('completed_at') and len(previous['imports'])==3, 'resume requires exactly three completed imports and no final result'
    assert previous['candidate_binary_sha256']==results['candidate_binary_sha256'] and previous['fixture']==meta
    assert all(i[s]['accepted_objects']==100000 and i[s]['exit_code']==0 for i in previous['imports'] for s in ('candidate','reference'))
    (run/'failed-attempt.json').write_text(json.dumps(previous,indent=2)+'\n')
    results['imports']=previous['imports']
    results['query_profile_rerun_reason']=args.resume_reason
    results['load_client_sha256']=hashlib.sha256(load.read_bytes()).hexdigest()
    config=run/'candidate-3.toml'
    assert config.is_file() and (run/'import-3.redb').is_file()
try:
    for rep in ([] if args.resume else range(1,4)):
        command(compose+['down','--volumes'])
        command(compose+['up','--no-build','-d','--wait','--wait-timeout','120'])
        command(compose+['exec','-T','reference','irrd_database_upgrade','--config','/etc/irrd.yaml'])
        config=write_config(rep)
        systems=['candidate','reference'] if rep%2==0 else ['reference','candidate']
        record={}
        for system in systems:
            if system=='reference':
                imported=json.loads(command(compose+['exec','-T','reference','python','-c',helper,'irrd_load_database','--config','/etc/irrd.yaml','--source','PERF','/fixture/perf.rpsl']))
                count=int(command(compose+['exec','-T','postgres','psql','-U','irrd','-d','irrd','-Atc',"SELECT count(*) FROM rpsl_objects WHERE source='PERF'"]))
            else:
                imported=json.loads(command(['taskset','-c','0-15',sys.executable,'-c',helper,str(candidate),'--config',str(config),'import','snapshot','--source','PERF','--file',str(fixture)]))
                count=json.loads(imported['stdout'])['objects']
            assert imported['exit_code']==0 and count==100000, 'import count or exit status differed'
            record[system]={k:v for k,v in imported.items() if k not in ['stdout','stderr']}
            record[system]['accepted_objects']=count
        results['imports'].append(record)
        save()
        print('imports',rep,{k:round(v['seconds'],3) for k,v in record.items()},flush=True)
    if args.resume:
        command(compose+['stop'])
        command(compose+['up','--no-build','-d','--wait','--wait-timeout','120'])
        assert int(command(compose+['exec','-T','postgres','psql','-U','irrd','-d','irrd','-Atc',"SELECT count(*) FROM rpsl_objects WHERE source='PERF'"]))==100000
    start=time.perf_counter()
    command(compose+['exec','-T','-d','reference','irrd','--config','/etc/irrd.yaml','--foreground'])
    ready(8000)
    command(compose+['exec','-T','redis','redis-cli','PUBLISH','irrd-preload-reload-channel','unknown-classes-changed-preload-all'])
    deadline=time.monotonic()+60
    while b'198.18.0.0/32' not in whois('!gAS4200000000'):
        assert time.monotonic()<deadline,'WHOIS preload readiness deadline'
        time.sleep(.05)
    results['reference_start_to_ready_seconds']=time.perf_counter()-start
    results['startup_readiness_scope']={'reference':'HTTP exact object plus WHOIS origin preload ready','candidate':'HTTP exact object ready; WHOIS listener bound before HTTP starts'}
    results['candidate_start_to_ready_seconds']=start_candidate(candidate,config)
    if args.before:
        results['before_binary_sha256']=hashlib.sha256(args.before.read_bytes()).hexdigest()
        for rep in range(1,4):
            versions=[('before',args.before),('current',candidate)]
            if rep%2==0: versions.reverse()
            for variant,binary in versions:
                start_candidate(binary,config)
                profile('candidate','graphql','reused',10,rep,variant,'optimization')
    start_candidate(candidate,config)
    for rep in range(1,4):
        systems=['reference','candidate'] if rep%2 else ['candidate','reference']
        for mode in ['whois','graphql']:
            for system in systems: profile(system,mode,'reused',args.seconds,rep)
    for mode in ['whois','graphql']:
        for system in ['reference','candidate']: profile(system,mode,'new',10,1)
    time.sleep(5)
    results['idle_pss']=memory()
    command(compose+['exec','-T','postgres','psql','-U','irrd','-d','irrd','-c','CHECKPOINT'])
    volume=json.loads(command(['docker','volume','inspect','rirrd-scale_postgres-data']))[0]['Mountpoint']
    results['disk']={'reference_allocated_bytes':int(command(['du','-s','-B1',volume]).split()[0]),'candidate_allocated_bytes':(run/'import-3.redb').stat().st_blocks*512,'candidate_apparent_bytes':(run/'import-3.redb').stat().st_size,
                     'scope':'persistent database allocation; PostgreSQL cluster and WAL versus redb file; images/logs excluded; Redis persistence disabled'}
    results['reference_image_id']=command(['docker','inspect','--format','{{.Image}}',command(compose+['ps','-q','reference'])])
    results['runtime_config_sha256']=hashlib.sha256((state/'irrd.yaml').read_bytes()).hexdigest()
    results['completed_at']=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())
    save()
    print('COMPLETE',run/'results.json',flush=True)
finally:
    stop_candidate()
    command(compose+['stop'])
