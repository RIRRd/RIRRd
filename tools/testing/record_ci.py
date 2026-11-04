#!/usr/bin/env python3
"""Attach verified GitHub workflow metadata without relabeling local case counts."""
import argparse
import datetime
import json
from pathlib import Path
import subprocess

REQUIRED={'Core · ubuntu-24.04','Core · ubuntu-24.04-arm','Core · windows-latest',
          'Isolated websites and browser workflows','Locked dependency security audit',
          'Source coverage and external protocol contracts','Validate workflow syntax and expressions'}


def proof(run,jobs,source,repo):
    if run.get('status')!='completed' or run.get('conclusion')!='success':
        raise ValueError('Workflow has not completed successfully')
    if run.get('head_sha')!=source or run.get('repository',{}).get('full_name')!=repo:
        raise ValueError('Workflow repository/source does not match the recorded snapshot')
    expected_url=f'https://github.com/{repo}/actions/runs/{run["id"]}'
    if run.get('html_url')!=expected_url:
        raise ValueError('Unexpected GitHub workflow URL')
    names={job['name'].rsplit(' / ',1)[-1] for job in jobs if job.get('status')=='completed' and job.get('conclusion')=='success'}
    if not REQUIRED<=names or any(job.get('status')!='completed' or job.get('conclusion')!='success' for job in jobs):
        raise ValueError('Missing or unsuccessful build/release jobs')
    return {'source_commit':source,'url':expected_url,'workflow':run['name'],'run_id':run['id'],
            'run_attempt':run['run_attempt'],'conclusion':'success',
            'checked_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'jobs':[{'name':job['name'],'conclusion':job['conclusion']}for job in jobs]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo',default='RIRRd/RIRRd');parser.add_argument('--run',type=int,required=True)
    parser.add_argument('--snapshot',type=Path,default=Path('site/project/verification.json'))
    parser.add_argument('--gh',default='gh',help='Authenticated CLI or private organization-scoped wrapper')
    args=parser.parse_args();snapshot=json.loads(args.snapshot.read_text())
    def api(path):return json.loads(subprocess.check_output([args.gh,'api',path],text=True))
    run=api(f'repos/{args.repo}/actions/runs/{args.run}');jobs=api(f'repos/{args.repo}/actions/runs/{args.run}/jobs?per_page=100')
    if jobs['total_count']!=len(jobs['jobs']):raise ValueError('Incomplete workflow job list')
    snapshot['verified_ci']=proof(run,jobs['jobs'],snapshot['source_commit'],args.repo)
    snapshot['ci_status']='verified_workflow'
    args.snapshot.write_text(json.dumps(snapshot,indent=2)+'\n')
    print('Attached verified CI workflow:',snapshot['verified_ci']['url'])

if __name__=='__main__':main()
