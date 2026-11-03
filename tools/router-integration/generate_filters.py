#!/usr/bin/env python3
"""Generate a new BIRD/FRR candidate bundle via bgpq4. Never changes a router."""
import argparse
import datetime
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import select
import time
import subprocess
import tempfile


def bounded_command(command):
    """Bound both runtime and output before allocating a complete bgpq4 response."""
    child=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    output=bytearray()
    deadline=time.monotonic()+30
    try:
        while True:
            remaining=deadline-time.monotonic()
            if remaining<=0 or not select.select([child.stdout],[],[],remaining)[0]:
                raise ValueError('bgpq4 timed out; candidate not published')
            block=os.read(child.stdout.fileno(),65536)
            if not block: break
            output.extend(block)
            if len(output)>8*1024*1024:
                raise ValueError('bgpq4 output exceeds the 8 MiB candidate budget')
        status=child.wait(timeout=max(.01,deadline-time.monotonic()))
        text=output.decode('utf-8')
        if status:
            raise ValueError('bgpq4 failed; candidate not published: '+text[:500].strip())
        return text
    finally:
        if child.poll() is None:
            child.kill(); child.wait()
        child.stdout.close()


def generate(args):
    if args.output.exists() or args.output.is_symlink():
        raise ValueError('output must be a new path; existing policies are never overwritten')
    if not re.fullmatch(r'(?:[A-Za-z0-9][A-Za-z0-9_.-]{0,252}|\[[0-9A-Fa-f:]+\]):[0-9]{1,5}', args.endpoint):
        raise ValueError('provide an explicit hostname:port or [IPv6]:port endpoint')
    if not 1 <= int(args.endpoint.rsplit(':', 1)[1]) <= 65535:
        raise ValueError('endpoint port is outside 1..65535')
    if not all(re.fullmatch(r'[A-Z][A-Z0-9_-]{0,31}', s) for s in args.sources.split(',')):
        raise ValueError('sources must be a comma-separated uppercase source list')
    if not re.fullmatch(r'AS[A-Z0-9:_-]{0,250}', args.object.upper()) or '::' in args.object:
        raise ValueError('provide one ASN or AS-set, without source-override notation')
    if not re.fullmatch(r'[A-Z][A-Z0-9_]{0,47}', args.name):
        raise ValueError('name must contain 1..48 uppercase letters, digits or underscores')
    if not 1 <= args.max_prefixes <= 65536:
        raise ValueError('max-prefixes must be between 1 and 65536')
    families=[4,6] if args.families=='both' else [int(args.families)]
    prefixes={}
    for family in families:
        command=[args.bgpq4, '-j', '-'+str(family), '-h', args.endpoint, '-S', args.sources, '-l', 'RIRRD_CANDIDATE']
        if args.allow_reserved_asns:
            command.append('-p')
        command.append(args.object.upper())
        rows=json.loads(bounded_command(command)).get('RIRRD_CANDIDATE')
        if not isinstance(rows,list) or not 1<=len(rows)<=args.max_prefixes:
            raise ValueError(f'IPv{family} returned an empty or over-budget list; retain the previous policy')
        networks=set()
        for row in rows:
            if not isinstance(row,dict) or row.get('exact') is not True:
                raise ValueError('only exact prefix matches are accepted by this generator')
            network=ipaddress.ip_network(row['prefix'],strict=True)
            if network.version!=family or network.prefixlen==0:
                raise ValueError('wrong address family or a default route in the candidate')
            networks.add(network)
        prefixes[str(family)]=[str(p) for p in sorted(networks,key=lambda p:(int(p.network_address),p.prefixlen))]
    bird=[]
    frr=[]
    for family,values in prefixes.items():
        name=args.name+'_'+family
        bird.append('define '+name+' = [\n    '+',\n    '.join(values)+'\n];\n')
        keyword='ip' if family=='4' else 'ipv6'
        frr.extend(f'{keyword} prefix-list {name} seq {index*5} permit {prefix}' for index,prefix in enumerate(values,1))
    files={'filters.bird':'\n'.join(bird),'filters.frr':'\n'.join(frr)+'\n','prefixes.json':json.dumps(prefixes,indent=2)+'\n'}
    manifest={'format_version':1,'generated_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'endpoint':args.endpoint,'sources':args.sources.split(','),'object':args.object.upper(),'name':args.name,'reserved_asns_allowed':args.allow_reserved_asns,'counts':{f:len(v) for f,v in prefixes.items()},'sha256':{name:hashlib.sha256(value.encode()).hexdigest() for name,value in files.items()}}
    files['manifest.json']=json.dumps(manifest,indent=2)+'\n'
    with tempfile.TemporaryDirectory(prefix='.rirrd-candidate-',dir=args.output.parent) as stage:
        stage=Path(stage)/'bundle'
        stage.mkdir(mode=0o700)
        for name,value in files.items():
            (stage/name).write_text(value,encoding='utf-8')
        # Reserve a new name, then replace our empty directory with a complete bundle.
        # A pre-existing file, directory or symlink causes mkdir to fail.
        args.output.mkdir(mode=0o700)
        try:
            os.replace(stage,args.output)
        except OSError:
            args.output.rmdir()  # Only removes our still-empty reservation.
            raise
    return manifest


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint',required=True)
    parser.add_argument('--sources',required=True)
    parser.add_argument('--object',required=True)
    parser.add_argument('--name',required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--families',choices=['4','6','both'],default='both')
    parser.add_argument('--max-prefixes',type=int,default=4096)
    parser.add_argument('--allow-reserved-asns',action='store_true',help='only for synthetic fixtures or deliberate reserved/private-AS policy')
    parser.add_argument('--bgpq4',default='bgpq4')
    args=parser.parse_args()
    try:
        print(json.dumps(generate(args)))
    except (OSError,ValueError,KeyError,subprocess.TimeoutExpired) as error:
        parser.exit(1,str(error)+'\n')

if __name__=='__main__':
    main()
