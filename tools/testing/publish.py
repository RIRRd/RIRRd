#!/usr/bin/env python3
"""Create a small public verification snapshot from matching, clean run reports."""
import argparse
import datetime
import json
from pathlib import Path
import re
import subprocess
from harness import ROOT


def checked(report):
    if report.get('source_dirty') is not False:raise ValueError('Do not publish verification from a dirty checkout')
    if report.get('origin') not in ['local','github_actions']:raise ValueError('Unknown execution origin')
    if report['origin']=='github_actions' and not re.fullmatch(r'https://github\.com/[^/]+/[^/]+/actions/runs/[0-9]+',report.get('run_url')or''):
        raise ValueError('GitHub-origin results need an actual workflow run URL')
    commit=report.get('source_commit','')
    if not re.fullmatch(r'[0-9a-f]{40}',commit):raise ValueError('Missing exact source revision')
    subprocess.run(['git','merge-base','--is-ancestor',commit,'HEAD'],cwd=ROOT,check=True,capture_output=True)
    totals={k:0 for k in ['tests','passed','failed','skipped']}
    for group in report['groups']:
        for key in totals:
            if type(group[key])is not int or group[key]<0:raise ValueError('Invalid result count')
            totals[key]+=group[key]
        if group['tests']!=group['passed']+group['failed']+group['skipped']:raise ValueError('Inconsistent case totals')
        if group['status']=='passed' and (group['failed'] or group['exit_code']!=0):raise ValueError('A failed gate cannot be labelled passed')
    status='passed' if report['groups'] and all(g['status']=='passed' for g in report['groups']) else 'failed'
    if totals!=report['counts']or status!=report['status']:raise ValueError('Summary does not match actual gates')
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--core',type=Path,required=True);parser.add_argument('--web',type=Path,required=True);parser.add_argument('--coverage',type=Path);parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args();reports=[checked(json.loads(path.read_text()))for path in [args.core,args.web]]
    commits={r['source_commit']for r in reports}
    if len(commits)!=1:raise ValueError('Run core and web against the same source revision')
    result={'schema_version':1,'recorded_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_commit':reports[0]['source_commit'],'status':'passed' if all(r['status']=='passed' for r in reports)else'failed','reports':reports,'coverage':None,'ci_status':'not_verified','ci_url':'https://github.com/Peravix/Rirrd/actions/workflows/ci.yml'}
    if args.coverage:
        coverage=json.loads(args.coverage.read_text())
        if coverage['source_dirty']or coverage['source_commit']not in commits:raise ValueError('Coverage must match the clean source revision')
        result['coverage']=coverage
    if all(r['origin']=='github_actions' and r['run_url']for r in reports):result['ci_status']='recorded_run'
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(result,indent=2)+'\n')


if __name__=='__main__':main()
