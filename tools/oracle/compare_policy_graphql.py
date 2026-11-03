#!/usr/bin/env python3
"""Check normal HTTP GraphQL values and field types for the policy/router fixture."""
import argparse
import json
from urllib.request import Request,urlopen

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference',default='http://127.0.0.1:8000')
parser.add_argument('--candidate',default='http://127.0.0.1:9080')
args=parser.parse_args()
cases=[
 ('RPSLFilterSet','AS64496:FLTR-DEMO','filterSet filter mpFilter descr adminC techC'),
 ('RPSLPeeringSet','AS64496:PRNG-DEMO','peeringSet peering mpPeering descr adminC techC'),
 ('RPSLRtrSet','AS64496:RTRS-DEMO','rtrSet members mpMembers mbrsByRef descr adminC techC'),
 ('RPSLInetRtr','R1.EXAMPLE','inetRtr localAs alias ifaddr interface peer mpPeer memberOf rsIn rsOut descr adminC techC'),
]
def request(base,query):
    req=Request(base+'/graphql/',data=json.dumps({'query':query}).encode(),headers={'Content-Type':'application/json'})
    with urlopen(req,timeout=15) as response:
        data=response.read(131073)
    assert len(data)<=131072
    value=json.loads(data)
    assert not value.get('errors'),'GraphQL returned errors'
    return value['data']
count=0
for typename,key,fields in cases:
    query='{ rpslObjects(rpslPk:["'+key+'"],sources:["DIFFERENTIAL"]) { ... on '+typename+' { '+fields+' } } }'
    ref,candidate=[request(base,query) for base in [args.reference,args.candidate]]
    assert len(ref['rpslObjects'])==1,'missing reference fixture'
    assert ref==candidate,f'{typename} values differ'
    query='{ __type(name:"'+typename+'") { fields { name type { kind name ofType { kind name ofType {kind name} } } } } }'
    types=[]
    for base in [args.reference,args.candidate]:
        data=request(base,query)
        types.append({f['name']:f['type'] for f in data['__type']['fields'] if f['name'] in fields.split()})
    assert set(types[0])==set(fields.split()) and types[0]==types[1],f'{typename} types differ'
    count+=len(fields.split())
print(f'Four policy/router HTTP GraphQL value cases and {count} field types match pinned IRRd')
