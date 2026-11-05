#!/usr/bin/env python3
"""Compare bounded inverse-filter values/types using the pinned synthetic fixture."""
import argparse
import json
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference', default='http://127.0.0.1:8000')
parser.add_argument('--candidate', default='http://127.0.0.1:9080')
args = parser.parse_args()


def query(base, document):
    request = Request(base + '/graphql/', data=json.dumps({'query': document}).encode(),
                      headers={'Content-Type': 'application/json'})
    with urlopen(request, timeout=15) as response:
        raw = response.read(131073)
    assert len(raw) <= 131072
    value = json.loads(raw)
    assert not value.get('errors'), value.get('errors')
    return value['data']


cases = [
    ('mntBy:["DIFFERENTIAL-MNT"],objectClass:["route","route6"]', 2),
    ('mntBy:["DIFFERENTIAL-MNT","DIFFERENTIAL-MNT"],objectClass:["aut-num"]', 1),
    ('mntBy:["DIFFERENTIAL-MNT"],rpslPk:["AS64496"]', 1),
    ('mntBy:["NONEXISTENT"],rpslPk:["AS64496"]', 0),
    ('mntBy:["NONEXISTENT","DIFFERENTIAL-MNT"],objectClass:["inet-rtr"]', 1),
    ('adminC:["DIFF1-TEST"],objectClass:["organisation","aut-num"]', 2),
    ('adminC:["DIFF1-TEST"],techC:["DIFF1-TEST"],objectClass:["aut-num"]', 1),
    ('mntBy:["DIFFERENTIAL-MNT"],techC:["NONEXISTENT"]', 0),
    ('zoneC:["DIFF1-TEST"]', 1),
    ('memberOf:["AS64496:RTRS-DEMO"]', 1),
    ('mbrsByRef:["ANY"]', 1),
    ('mntBy:["differential-mnt"],objectClass:["route"]', 1),
    ('mpMembers:["AS64496:RTRS-CHILD"]', 1),
    ('role:["NONEXISTENT"]', 0),
    ('origin:["AS64496"]', 2),
]
for arguments, count in cases:
    document = '{rpslObjects(sources:["DIFFERENTIAL"],' + arguments + ') {rpslPk objectClass source}}'
    rows = [query(base, document)['rpslObjects'] for base in [args.reference, args.candidate]]
    assert len(rows[0]) == count, (arguments, 'unexpected reference fixture count')
    assert sorted(rows[0], key=lambda row: json.dumps(row, sort_keys=True)) == sorted(
        rows[1], key=lambda row: json.dumps(row, sort_keys=True)), arguments

document = '{__type(name:"Query"){fields{name args{name type{kind name ofType{kind name ofType{kind name}}}}}}}'
fields = {'mntBy', 'adminC', 'techC', 'zoneC', 'memberOf', 'mbrsByRef', 'members', 'mpMembers', 'person', 'role', 'origin'}
types = []
for base in [args.reference, args.candidate]:
    objects = next(field for field in query(base, document)['__type']['fields'] if field['name'] == 'rpslObjects')
    types.append({arg['name']: arg['type'] for arg in objects['args'] if arg['name'] in fields})
assert set(types[0]) == fields and types[0] == types[1]
print(f'{len(cases)} inverse GraphQL comparisons and {len(fields)} argument types match pinned IRRd')
