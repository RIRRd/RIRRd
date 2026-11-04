#!/usr/bin/env python3
"""Read-only repository preflight using an organization-scoped GH_TOKEN."""
import argparse
import json
import subprocess
import sys

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--repo',default='RIRRd/RIRRd')
parser.add_argument('--branch',default='main')
args=parser.parse_args()
def gh(*arguments):
    result=subprocess.run(['gh',*arguments],text=True,capture_output=True)
    if result.returncode:
        print(result.stderr.strip(),file=sys.stderr)
        raise SystemExit(result.returncode)
    return result.stdout

repo=json.loads(gh('api','repos/'+args.repo))
print(json.dumps({'repository':repo['full_name'],'private':repo['private'],
    'default_branch':repo['default_branch'],'issues_enabled':repo['has_issues'],
    'can_administer':repo.get('permissions',{}).get('admin',False)},indent=2))
print(gh('pr','list','--repo',args.repo,'--json','number,url,state,statusCheckRollup'))
print(gh('run','list','--repo',args.repo,'--branch',args.branch,'--limit','5','--json','databaseId,url,conclusion,status'))
