#!/usr/bin/env python3
"""Read-only publication preflight; never changes visibility or bypasses review."""
import json
import subprocess
import sys

REPO='Peravix/Rirrd'
def gh(*arguments):
    result=subprocess.run(['gh',*arguments],text=True,capture_output=True)
    if result.returncode:
        print(result.stderr.strip(),file=sys.stderr)
        raise SystemExit(result.returncode)
    return result.stdout

gh('auth','status')
repo=json.loads(gh('api','repos/'+REPO))
print(json.dumps({'repository':repo['full_name'],'private':repo['private'],
    'default_branch':repo['default_branch'],'issues_enabled':repo['has_issues'],
    'can_administer':repo.get('permissions',{}).get('admin',False)},indent=2))
print(gh('pr','list','--repo',REPO,'--head','release/v0.1.0-beta.1','--json','number,url,state,statusCheckRollup'))
print(gh('run','list','--repo',REPO,'--branch','release/v0.1.0-beta.1','--limit','5','--json','databaseId,url,conclusion,status'))
