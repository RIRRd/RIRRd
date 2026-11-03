#!/usr/bin/env python3
"""Benchmark the normal HTTP history route using synthetic 24-hour telemetry."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import http.client
import importlib.util
import json
import multiprocessing
import os
from pathlib import Path
import resource
import shutil
import subprocess
import threading
import time

ROOT=Path(__file__).resolve().parents[2]
def serve(file,pipe,cpu):
    os.sched_setaffinity(0,{cpu})
    spec=importlib.util.spec_from_file_location('history_bench_gateway',file);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    state=module.State(Path('/unused-benchmark-dataset'),1)
    now=time.time()
    for n in range(1440):state.history.append({'timestamp':now-60*(1439-n),'objects':100000+n,'rss_bytes':400000000,'cpu_percent':.1,'http_qps':.25,'http_p95_ms':1.5,'vrps':1000000})
    server=module.Server(('127.0.0.1',0),state);threading.Thread(target=server.serve_forever,daemon=True).start();pipe.send(server.server_port)
    while True:
        cmd=pipe.recv()
        if cmd=='start':started=time.process_time();pipe.send(True)
        elif cmd=='end':pipe.send({'cpu_seconds':time.process_time()-started,'peak_rss_bytes':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*1024})
        else:server.shutdown();server.server_close();break

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--baseline',default='v0.1.0-beta.1');parser.add_argument('--work',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
    work=args.work.resolve()
    if work.is_relative_to(ROOT):parser.error('Keep runtime fixtures outside Git')
    work.mkdir(parents=True,exist_ok=True)
    commit=subprocess.check_output(['git','rev-parse',args.baseline+'^{commit}'],cwd=ROOT,text=True).strip()
    (work/'baseline.py').write_bytes(subprocess.check_output(['git','show',commit+':tools/live-service/gateway.py'],cwd=ROOT));shutil.copyfile(ROOT/'tools/live-service/gateway.py',work/'candidate.py');shutil.copyfile(ROOT/'tools/live-service/node.json',work/'node.json')
    context=multiprocessing.get_context('spawn');cpu=max(os.sched_getaffinity(0));servers={}
    try:
        for name in ['baseline','candidate']:
            parent,child=context.Pipe();process=context.Process(target=serve,args=(str(work/(name+'.py')),child,cpu));process.start();servers[name]=(process,parent,parent.recv())
        def request(port):
            connection=http.client.HTTPConnection('127.0.0.1',port,timeout=10);connection.request('GET','/dashboard/history.json');response=connection.getresponse();body=response.read();assert response.status==200;assert len(body)==int(response.getheader('Content-Length'));connection.close();return len(body)
        rows=[]
        for repetition in range(3):
            for name in ['baseline','candidate'] if repetition%2==0 else ['candidate','baseline']:
                process,pipe,port=servers[name]
                for _ in range(10):request(port)
                pipe.send('start');pipe.recv();started=time.monotonic()
                with ThreadPoolExecutor(max_workers=4) as pool: sizes=list(pool.map(lambda _:request(port),range(400)))
                elapsed=time.monotonic()-started;pipe.send('end');observed=pipe.recv();rows.append({'round':repetition,'implementation':name,'requests':len(sizes),'wall_seconds':elapsed,'response_bytes':sizes[0],**observed})
        # Response shape and observation values are unchanged; timestamps differ
        # because each isolated fixture is created in its own process.
        args.output.write_text(json.dumps({'kind':'loopback HTTP history; 1440 synthetic samples, four clients; server CPU excludes client','baseline_commit':commit,'candidate_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'candidate_source_sha256':hashlib.sha256((work/'candidate.py').read_bytes()).hexdigest(),'cpu':cpu,'samples':rows},indent=2)+'\n')
        before=sum(r['cpu_seconds'] for r in rows if r['implementation']=='baseline');after=sum(r['cpu_seconds'] for r in rows if r['implementation']=='candidate');print('Server CPU reduction:',round(100*(1-after/before),1),'percent');print('Results:',args.output)
    finally:
        for process,pipe,port in servers.values():
            try:pipe.send('stop')
            except (BrokenPipeError,EOFError):pass
            process.join(10)
            if process.is_alive():process.terminate();process.join()
if __name__=='__main__':main()
