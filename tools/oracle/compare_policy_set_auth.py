#!/usr/bin/env python3
"""Compare real HTTP authorization decisions on isolated synthetic AUTH-TEST sources."""
import argparse
import json
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference', default='http://127.0.0.1:8000')
parser.add_argument('--candidate', default='http://127.0.0.1:9080')
args = parser.parse_args()
owner, parent = 'oracle owner proof', 'oracle parent proof'
# Deterministic synthetic bcrypt vector, not a service credential.
SYNTHETIC_HASH = '$2b$04$abcdefghijklmnopqrstuubqfXErTEG3oR7IEslzxdJFrAj1MAcjK'
checks = 0

def check(method, text, passwords, expected):
    global checks
    payload = {'objects': [{'object_text': text}], 'passwords': passwords}
    if method == 'DELETE':
        payload['delete_reason'] = 'synthetic oracle cleanup'
    data = json.dumps(payload).encode()
    for label, base in [('reference', args.reference), ('candidate', args.candidate)]:
        req = Request(base.rstrip('/')+'/v1/submit/', data=data, method=method,
                      headers={'Content-Type':'application/json'})
        with urlopen(req, timeout=15) as response:
            body = response.read(262145)
        assert len(body) <= 262144
        assert all(secret.encode() not in body for secret in [owner, parent, 'wrong proof', SYNTHETIC_HASH])
        report = json.loads(body)
        assert report['summary'][expected] == 1, f'{label}: case {checks+1} expected {expected}'
    checks += 1

for cls, prefix, attributes in [
    ('filter-set', 'FLTR', 'filter: AS64496\n'),
    ('peering-set', 'PRNG', 'peering: AS64496\n'),
    ('rtr-set', 'RTRS', 'members: router.example, AS64496:RTRS-CHILD\nmbrs-by-ref: MISSING-MNT\n'),
]:
    text = f'{cls}: AS64496:{prefix}-ORACLE\n{attributes}admin-c: CT1-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
    for passwords in [['wrong proof'], [owner], [parent]]:
        check('POST', text, passwords, 'failed')
    check('POST', text, [owner,parent], 'successful_create')
    check('POST', text+'remarks: Updated through HTTP\n', [owner], 'successful_modify')
    check('POST', text, [parent], 'failed')
    check('POST', text.replace('CT1-AUTH','MISSING-AUTH'), [owner], 'failed')
    check('POST', text.replace('source: AUTH-TEST','source: AUTH-OTHER'), [owner,parent], 'failed')
    check('DELETE', text, [parent], 'failed')
    check('DELETE', text, [owner], 'successful_delete')
router = 'inet-rtr: router.example\nlocal-as: AS64496\nmember-of: AS64496:RTRS-MISSING\nadmin-c: CT1-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
check('POST',router,['wrong proof'],'failed')
check('POST',router.replace('local-as: AS64496\n',''),[owner],'failed')
check('POST',router,[owner],'successful_create')
check('POST',router.replace('CT1-AUTH','MISSING-AUTH'),[owner],'failed')
check('POST',router.replace('local-as: AS64496','local-as: bad-asn'),[owner],'failed')
check('POST',router.replace('source: AUTH-TEST','source: AUTH-OTHER'),[owner,parent],'failed')
check('POST',router,[parent],'failed')
check('POST',router+'alias: alias.example\n',[owner],'successful_modify')
check('DELETE',router,[parent],'failed')
check('DELETE',router,[owner],'successful_delete')
dummy = 'mntner: OWNER-MNT\nadmin-c: CT1-AUTH\nupd-to: oracle@example.invalid\nauth: BCRYPT-PW DummyValue\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
check('POST',dummy,['wrong proof'],'failed')
check('POST',dummy,[owner,parent],'failed')
check('POST',dummy,[owner,owner],'successful_modify')
check('POST',dummy+f'auth: BCRYPT-PW {SYNTHETIC_HASH}\n',[owner],'failed')
check('POST',dummy.replace('OWNER-MNT','NEW-MNT'),[owner],'failed')
check('POST',dummy,[owner],'successful_modify')
check('POST',dummy+'auth: BCRYPT-PW DummyValue\n',[owner],'successful_modify')
print(f'{checks} authoritative HTTP authorization cases match pinned IRRd (both implementations checked)')
