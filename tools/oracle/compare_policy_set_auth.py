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
    objects = [text] if isinstance(text, str) else text
    payload = {'objects': [{'object_text': item} for item in objects], 'passwords': passwords}
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
        assert report['summary'][expected] == len(objects), f'{label}: case {checks+1} expected {expected}'
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
# Additional bounded class profiles; no DNS queries or live delegation changes.
zone = 'person: Zone Contact\naddress: Example Road\nphone: +1 202 555 0100\ne-mail: zone@example.invalid\nnic-hdl: ZONE1-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
check('POST', zone, [owner], 'successful_create')
for head in ['as-block: AS64496 - AS64511\n', 'domain: 2.0.192.in-addr.arpa\nzone-c: ZONE1-AUTH\nnserver: ns.example.invalid\n']:
    text = head+'admin-c: CT1-AUTH\ntech-c: CT1-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
    check('POST',text,['wrong proof'],'failed')
    check('POST',text,[parent],'failed')
    check('POST',text.replace('tech-c: CT1-AUTH\n',''),[owner],'failed')
    check('POST',text.replace('CT1-AUTH','MISSING-AUTH'),[owner],'failed')
    check('POST',text.replace('source: AUTH-TEST','source: AUTH-OTHER'),[owner],'failed')
    check('POST',text,[owner],'successful_create')
    check('POST',text+'remarks: Changed\n',[owner],'successful_modify')
    check('POST',text+'auth: DUMMY\n',[owner],'failed')
    if head.startswith('domain:'):
        check('POST',text.replace('ZONE1-AUTH','MISSING-AUTH'),[owner],'failed')
        check('DELETE',zone,[owner],'failed')
    check('DELETE',text,[parent],'failed')
    check('DELETE',text,[owner],'successful_delete')
check('DELETE',zone,[owner],'successful_delete')
# Protected names include dangling inbound references, not just tombstones.
for kind in ['person', 'role']:
    claimed = (f'{kind}: Claimed Contact\naddress: Example Road\nphone: +1 202 555 0100\n'
               'e-mail: contact@example.invalid\nnic-hdl: DANGLING-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n')
    check('POST', claimed, [owner], 'failed')
# Batch deletions may remove a contact and all of its referring objects together.
for kind, handle in [('person', 'BATCH-PERSON-AUTH'), ('role', 'BATCH-ROLE-AUTH')]:
    contact = (f'{kind}: Batch Contact\naddress: Example Road\nphone: +1 202 555 0100\n'
               f'e-mail: contact@example.invalid\nnic-hdl: {handle}\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n')
    domain = (f'domain: batch-{kind}.example\nadmin-c: CT1-AUTH\ntech-c: CT1-AUTH\n'
              f'zone-c: {handle}\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n')
    extra = domain.replace(f'batch-{kind}.example', f'extra-{kind}.example')
    check('POST', [contact, domain, extra], [owner], 'successful_create')
    check('DELETE', contact, [owner], 'failed')
    check('DELETE', [contact, domain, extra], ['wrong proof'], 'failed')
    check('DELETE', extra, [owner], 'successful_delete')
    check('DELETE', [contact, domain], [owner], 'successful_delete')
    check('POST', contact, [owner], 'failed')
print(f'{checks} authoritative HTTP authorization cases match pinned IRRd (both implementations checked)')
