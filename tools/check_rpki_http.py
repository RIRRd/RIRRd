#!/usr/bin/env python3
"""Read-only public beta checks for RPKI status, explanations and batch failures."""
import argparse
import ipaddress
import json
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request,urlopen

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url',required=True)
args=parser.parse_args();base=args.url.rstrip('/')
def request(path,body=None):
    req=Request(base+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    try:
        with urlopen(req,timeout=20) as r:return r.status,json.load(r)
    except HTTPError as e:return e.code,e.read().decode()
code,status=request('/v1/rpki/status');assert code==200 and status['usable'],status
assert status['vrps_ipv4']+status['vrps_ipv6']==status['vrps']
assert sum(a['vrps_ipv4'] for a in status['trust_anchors'])==status['vrps_ipv4']
assert sum(a['vrps_ipv6'] for a in status['trust_anchors'])==status['vrps_ipv6']
assert 'rtr_listen' not in status
code,valid=request('/v1/rpki/validate?'+urlencode({'prefix':'193.0.0.0/21','origin':'AS3333'}))
assert code==200 and valid['state']=='VALID',valid
assert 0<len(valid['covering_roas'])<=32
assert any(r['asn']==3333 and ipaddress.ip_network('193.0.0.0/21').subnet_of(ipaddress.ip_network(r['prefix'])) and r['max_length']>=21 for r in valid['covering_roas'])
code,batch=request('/v1/rpki/validate',{'routes':[{'prefix':'193.0.0.0/21','origin':'AS3333'},{'prefix':'193.0.0.0/21','origin':'AS64496'}]})
assert code==200,batch
assert [r['state'] for r in batch['results']]==['VALID','INVALID'],batch
assert isinstance(batch['dataset_serial'],int)
for body in [{'routes':[]},{'routes':[{'prefix':'malformed','origin':'AS3333'}]},{'routes':[{'prefix':'193.0.0.0/21','origin':'AS3333'}]*101}]:
    code,data=request('/v1/rpki/validate',body);assert code==400,(code,data)
print(json.dumps({'vrps':status['vrps'],'ipv4':status['vrps_ipv4'],'ipv6':status['vrps_ipv6'],'anchors':len(status['trust_anchors']),'rejected_publication_points':status['rejected_publication_points'],'validation_seconds':status['validation_seconds'],'checks':'covering ROAs, atomic batch VALID/INVALID, malformed/empty/oversized route counts rejected'}))
