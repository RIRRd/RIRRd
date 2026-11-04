#!/usr/bin/env python3
"""Run build gates locally or in CI and always emit JSON, JUnit and a summary."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time

from harness import ROOT
from results import write


def rust_cases(text):
    records=[];target='rust'
    for line in text.splitlines():
        if 'Running ' in line and '(' in line:
            target=line.strip().split('Running ',1)[1].split(' (',1)[0]
        match=re.fullmatch(r'test (\S+) \.\.\. (ok|FAILED|ignored(?:,.*)?)',line)
        if match:
            name,status=match.groups();records.append({'name':target+'::'+name,'status':'passed' if status=='ok' else 'failed' if status=='FAILED' else 'skipped','seconds':0.0})
    return records


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile',choices=['core','web'],default='core')
    parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args();out=args.out.resolve()
    if out.is_relative_to(ROOT):parser.error('Keep reports, logs and fixtures outside the source repository')
    out.mkdir(parents=True,exist_ok=True)
    # A failed rerun must not leave an earlier successful artifact in place.
    for name in ['report.json','summary.md','junit.json','junit.xml',*[base+ext for base in ['protocol-cases','report-cases','gateway-cases','community-cases','web-cases'] for ext in ['.json','.xml']]]:
        (out/name).unlink(missing_ok=True)
    report={'schema_version':1,'profile':args.profile,'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'source_dirty':bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT)),'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'platform':platform.system()+' '+platform.machine(),'origin':'github_actions' if os.environ.get('GITHUB_ACTIONS')=='true' else 'local','run_url':None,'groups':[]}
    if report['origin']=='github_actions':
        report['run_url']='https://github.com/'+os.environ['GITHUB_REPOSITORY']+'/actions/runs/'+os.environ['GITHUB_RUN_ID']
    records=[]
    def group(name,command,case_file=None,rust=False,timeout=1200):
        start=time.monotonic();log=out/(name+'.log');code=1;output='';detail=''
        # New process groups let timeout cleanup include test daemons/compilers.
        kwargs={'start_new_session':True} if os.name!='nt' else {'creationflags':subprocess.CREATE_NEW_PROCESS_GROUP}
        try:
            with log.open('wb') as stream:
                process=subprocess.Popen(command,cwd=ROOT,stdout=stream,stderr=subprocess.STDOUT,**kwargs)
                try:code=process.wait(timeout=timeout)
                except (subprocess.TimeoutExpired,KeyboardInterrupt):
                    if os.name=='nt':subprocess.run(['taskkill','/PID',str(process.pid),'/T','/F'],capture_output=True,check=False)
                    else:os.killpg(process.pid,signal.SIGKILL)
                    process.wait();detail='Timed out or interrupted';code=124
            output=log.read_text(encoding='utf-8',errors='replace')
        except OSError as error:
            detail=str(error);log.write_text(detail)
        cases=rust_cases(output) if rust else []
        if case_file and case_file.with_suffix('.json').exists():
            try:
                cases=json.loads(case_file.with_suffix('.json').read_text())
                if not isinstance(cases,list) or any(not isinstance(c,dict) or not isinstance(c.get('name'),str) or c.get('status') not in ['passed','failed','skipped'] for c in cases):
                    raise ValueError('Invalid case records')
            except (OSError,ValueError,TypeError) as error:
                cases=[];code=1;detail='Invalid case report: '+str(error)
        if (rust or case_file) and not cases and code==0:
            code=1;detail='Runner returned success without test records'
        if any(c['status']=='failed' for c in cases):code=1
        item={'id':name,'status':'passed' if code==0 else 'failed','seconds':round(time.monotonic()-start,3),'exit_code':code,'tests':len(cases),'passed':sum(c['status']=='passed' for c in cases),'failed':sum(c['status']=='failed' for c in cases),'skipped':sum(c['status']=='skipped' for c in cases)}
        report['groups'].append(item);records.extend(cases)
        if not cases:
            records.append({'name':'build-gate::'+name,'status':item['status'],'seconds':item['seconds'],'detail':detail or ('See '+log.name if code else '')})
        print(f"{name}: {item['status']} ({item['passed']} passed cases, {item['skipped']} skipped; {item['seconds']}s)",flush=True)
        if code:print(output[-6000:],flush=True)
        return code==0
    python=sys.executable
    def built_binary():
        metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1','--locked'],cwd=ROOT,text=True))
        return Path(metadata['target_directory'])/'debug'/('rust-irrd.exe' if os.name=='nt' else 'rust-irrd')
    if args.profile=='core':
        group('format',['cargo','fmt','--all','--','--check'])
        group('clippy',['cargo','clippy','--locked','--all-targets','--','-D','warnings'])
        built=group('rust',['cargo','test','--locked','--all-targets'],rust=True)
        if built:
            binary=built_binary()
            group('protocols',[python,'tools/testing/protocols.py','--binary',str(binary),'--report',str(out/'protocol-cases')],case_file=out/'protocol-cases')
        else:
            report['groups'].append({'id':'protocols','status':'blocked','tests':0,'passed':0,'failed':0,'skipped':0,'seconds':0,'exit_code':None})
        group('report-exporter',[python,'tools/testing/unit.py','--suite','reports','--report',str(out/'report-cases')],case_file=out/'report-cases')
        if sys.platform.startswith('linux'):
            for suite in ['gateway','community']:
                group(suite,[python,'tools/testing/unit.py','--suite',suite,'--report',str(out/(suite+'-cases'))],case_file=out/(suite+'-cases'))
    else:
        built=group('web-build',['cargo','build','--locked'])
        if built:
            binary=built_binary()
            group('web',[python,'tools/testing/web.py','--binary',str(binary),'--work',str(out/'web-state'),'--report',str(out/'web-cases')],case_file=out/'web-cases',timeout=300)
    report['completed_at']=datetime.datetime.now(datetime.timezone.utc).isoformat()
    report['status']='passed' if all(g['status']=='passed' for g in report['groups']) else 'failed'
    report['counts']={key:sum(g[key] for g in report['groups']) for key in ['tests','passed','failed','skipped']}
    # Per-case durations are available for Python; stable cargo output does not
    # expose them, so Rust cases use 0 and the measured group time is authoritative.
    (out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    write(out/'junit',records)
    summary=f"## RIRRd {args.profile}: {report['status']}\n\nSource `{report['source_commit']}` · {report['origin']} · {report['platform']}\n\n| Gate | Result | Passed cases | Failed | Skipped | Seconds |\n| --- | --- | ---: | ---: | ---: | ---: |\n"
    summary+='\n'.join(f"| {g['id']} | {g['status']} | {g['passed']} | {g['failed']} | {g['skipped']} | {g['seconds']} |" for g in report['groups'])
    summary+='\n\nSynthetic isolated fixtures. Coverage is evidence for exercised behavior; full IRRd parity remains open.\n'
    (out/'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'],'a')as f:f.write(summary)
    return 0 if report['status']=='passed' else 1


if __name__=='__main__':raise SystemExit(main())
