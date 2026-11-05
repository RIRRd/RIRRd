#!/usr/bin/env python3
"""Compare allocation/IRT submissions against isolated pinned IRRd; synthetic data only."""
import argparse
import json
import re
import uuid
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference', default='http://127.0.0.1:8000')
parser.add_argument('--candidate', default='http://127.0.0.1:9080')
parser.add_argument('--report', type=Path)
parser.add_argument('--case-id', default=uuid.uuid4().hex[:8].upper(),
                    help='contact-handle suffix; recorded for reproduction on a clean fixture')
args = parser.parse_args()
assert re.fullmatch('[A-Za-z0-9]{1,16}', args.case_id), 'case-id must be 1-16 alphanumeric characters'
OWNER, PARENT = 'oracle owner proof', 'oracle parent proof'
peers = [('reference', args.reference), ('candidate', args.candidate)]
results = []

def post(base, path, payload, method='POST'):
    request = Request(base.rstrip('/')+path, data=json.dumps(payload).encode(), method=method,
                      headers={'Content-Type': 'application/json'})
    try:
        response = urlopen(request, timeout=20)
    except HTTPError as error:
        response = error
    with response:
        status, body = response.status, response.read(1048577)
    assert len(body) <= 1048576
    for secret in [OWNER, PARENT, 'wrong proof', 'synthetic-secret']:
        assert secret.encode() not in body, ('credential disclosure', path, status)
    return status, body

def change(name, texts, passwords, expected, delete=False, reference_http_status=None):
    objects = [texts] if isinstance(texts, str) else texts
    payload = {'objects': [{'object_text': text} for text in objects], 'passwords': passwords}
    if delete:
        payload['delete_reason'] = 'synthetic allocation test cleanup'
    replies = {}
    for label, base in peers:
        status, body = post(base, '/v1/submit/', payload, 'DELETE' if delete else 'POST')
        if label == 'reference' and reference_http_status is not None:
            assert expected == 'failed' and status == reference_http_status, (name,label,status,body)
            replies[label] = f'HTTP {status} pinned malformed-URL exception'
            continue
        # RIRRd rejects malformed RPSL before creating an object report; IRRd
        # includes parser errors in its HTTP 200 report. Both must fail closed.
        if status == 400:
            assert expected == 'failed', (name,label,status,body)
            replies[label] = 'HTTP 400 malformed submission'
        else:
            assert status == 200, (name,label,status,body)
            report = json.loads(body)
            assert report['summary'][expected] == len(objects), (name,label,expected,report)
            replies[label] = expected
    results.append({'name':name,'expected':expected,'replies':replies})

def query(name, keys, expected):
    query = '{rpslObjects(sources:["AUTH-TEST"],rpslPk:'+json.dumps(keys)+'){rpslPk objectClass objectText}}'
    for label, base in peers:
        status, body = post(base, '/graphql/', {'query':query})
        reply = json.loads(body)
        assert status == 200 and not reply.get('errors'), (name,label,status,reply)
        rows = reply['data']['rpslObjects']
        assert len(rows) == expected, (name,label,rows)
        assert not any('must roll back' in row['objectText'] for row in rows)
    results.append({'name':name,'expected_objects':expected})

# Verify both candidates are reachable before the first mutating comparison.
# An absent candidate must not leave a half-applied reference case behind.
for label, base in peers:
    status, body = post(base, '/graphql/', {
        'query':'{asnPrefixes(asns:[64496],sources:["AUTH-TEST"]){asn}}'
    })
    assert status == 200 and not json.loads(body).get('errors'), (label,status,body)

# The exact corpus is also consumed by Rust unit tests and the upstream parser.
for case in json.loads(Path('tests/fixtures/authoritative-allocations.json').read_text()):
    change('schema '+case['name'], case['text'], [OWNER],
           'successful_create' if case['valid'] else 'failed',
           reference_http_status=case.get('reference_http_status'))
    if case['valid']:
        change('schema cleanup '+case['name'], case['text'], [OWNER], 'successful_delete', True)

allocation_fields = 'netname: Oracle network\ncountry: GB\nadmin-c: HANDLE\ntech-c: HANDLE\nstatus: ASSIGNED PA\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
objects = [
    ('inetnum', '198.18.32.0 - 198.18.32.255', 'inetnum: 198.18.32.0 - 198.18.32.255\n'+allocation_fields),
    ('inet6num', '2001:DB8:CDEF::/48', 'inet6num: 2001:db8:cdef::/48\n'+allocation_fields),
    ('irt', 'IRT-AUTH-ORACLE', 'irt: IRT-AUTH-ORACLE\naddress: Documentation Road\ne-mail: noc@example.invalid\nabuse-mailbox: abuse@example.invalid\nadmin-c: HANDLE\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'),
]
for cls, key, template in objects:
    # Deleted contact handles are deliberately protected permanently. A unique
    # run suffix permits reruns without deleting that protection from the oracle.
    handle = 'ALLOC-'+cls.upper()+'-'+args.case_id.upper()
    contact = (f'person: Allocation Contact\naddress: Documentation Road\nphone: +1 202 555 0100\n'
               f'e-mail: noc@example.invalid\nnic-hdl: {handle}\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n')
    text = template.replace('HANDLE',handle)
    change(cls+' contact', contact, [OWNER], 'successful_create')
    change(cls+' wrong proof', text, ['wrong proof'], 'failed')
    change(cls+' create', text, [OWNER], 'successful_create')
    change(cls+' cross source', text.replace('source: AUTH-TEST','source: AUTH-OTHER'), [OWNER,PARENT], 'failed')
    change(cls+' missing reference', text.replace(handle,'MISSING-ALLOC-AUTH'), [OWNER], 'failed')
    change(cls+' protected contact', contact, [OWNER], 'failed', True)
    moved = text.replace('mnt-by: OWNER-MNT','mnt-by: PARENT-MNT')
    change(cls+' missing new proof', moved, [OWNER], 'failed')
    change(cls+' missing old proof', moved, [PARENT], 'failed')
    change(cls+' transfer', moved, [OWNER,PARENT], 'successful_modify')
    change(cls+' old owner cannot delete', moved, [OWNER], 'failed', True)
    change(cls+' current owner modifies', moved+'remarks: Changed by current maintainer\n', [PARENT], 'successful_modify')
    query(cls+' persists', [key], 1)
    # A same-source allocation newly created through the HTTP API must govern
    # route admission; the route owner alone cannot authorize the parent.
    if cls in ['inetnum','inet6num']:
        prefix = '198.18.32.0/24' if cls == 'inetnum' else '2001:db8:cdef::/48'
        route = f'{"route" if cls == "inetnum" else "route6"}: {prefix}\norigin: AS64496\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n'
        change(cls+' route without parent', route, [OWNER], 'failed')
        change(cls+' route with parent', route, [OWNER,PARENT], 'successful_create')
        change(cls+' route cleanup', route, [OWNER], 'successful_delete', True)
    change(cls+' deletion', moved, [PARENT], 'successful_delete', True)
    change(cls+' contact cleanup', contact, [OWNER], 'successful_delete', True)
    query(cls+' deletion persists', [key,handle], 0)

report={'status':'passed','oracle_commit':'8e8080dab744a348b7d0e6efcb53f403301dd075',
        'case_id':args.case_id,
        'checks':len(results),'results':results,
        'scope':'Synthetic schema/auth/reference/route-parent cases. CLI/HTTP atomic rollback is tested separately because upstream submissions may partially succeed.'}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
print(f'{len(results)} allocation/IRT HTTP and query case groups matched the pinned oracle')
