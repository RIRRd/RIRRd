#!/usr/bin/env python3
"""Compare supported organisation fields and their introspection types."""
import argparse
import json
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference', default='http://127.0.0.1:8000')
parser.add_argument('--candidate', default='http://127.0.0.1:9080')
args = parser.parse_args()
fields = {'organisation', 'orgName', 'country', 'address', 'phone', 'faxNo', 'eMail', 'adminC', 'techC'}
queries = [
    '{ rpslObjects(rpslPk:["ORG-DIFFERENTIAL"]) { rpslPk source ... on RPSLOrganisation { organisation orgName country address phone faxNo eMail adminC techC } } }',
    '{ __type(name:"RPSLOrganisation") { fields { name type { kind name ofType { kind name ofType { kind name } } } } } }',
]

def request(base, query):
    req = Request(base.rstrip('/') + '/graphql/', data=json.dumps({'query': query}).encode(),
                  headers={'Content-Type': 'application/json'})
    with urlopen(req, timeout=10) as response:
        body = response.read(131073)
    if len(body) > 131072:
        raise ValueError('oversized GraphQL response')
    value = json.loads(body)
    if value.get('errors'):
        raise ValueError('GraphQL returned errors')
    return value['data']

for i, query in enumerate(queries):
    reference, candidate = (request(base, query) for base in [args.reference, args.candidate])
    if i:
        reference, candidate = ({f['name']: f['type'] for f in data['__type']['fields'] if f['name'] in fields}
                                for data in [reference, candidate])
        assert set(reference) == fields
    assert reference == candidate, f'organisation GraphQL comparison {i+1} differed'
print('Organisation GraphQL values and nine field types match pinned upstream')
