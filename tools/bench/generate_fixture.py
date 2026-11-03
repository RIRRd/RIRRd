#!/usr/bin/env python3
"""Generate a deterministic, synthetic 100,000-object benchmark fixture (no real registry data)."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
args = parser.parse_args()
# RFC 6996 private-use ASNs, RFC 2544 IPv4 benchmark addresses, RFC 3849 IPv6.
common = 'admin-c: BENCH1-TEST\ntech-c: BENCH1-TEST\nmnt-by: BENCH-MNT\nsource: PERF\n\n'
with args.output.open('x', encoding='ascii', newline='\n') as out:
    out.write('person: Benchmark Contact\naddress: Synthetic benchmark only\nphone: +1 202 555 0100\ne-mail: bench@example.invalid\nnic-hdl: BENCH1-TEST\nmnt-by: BENCH-MNT\nsource: PERF\n\n')
    out.write('mntner: BENCH-MNT\nadmin-c: BENCH1-TEST\nupd-to: bench@example.invalid\nauth: BCRYPT-PW DummyValue\nmnt-by: BENCH-MNT\nsource: PERF\n\n')
    out.write('as-set: AS4200000000:AS-BENCH\nmembers: AS4200000000, AS4200000001\n'+common)
    for i in range(50000):
        out.write(f'aut-num: AS{4200000000+i}\nas-name: AS-BENCH-{i}\ndescr: Synthetic benchmark record {i}\n'+common)
    for i in range(25000):
        out.write(f'route: 198.{18+i//65536}.{(i//256)%256}.{i%256}/32\norigin: AS{4200000000+i}\ndescr: Synthetic IPv4 benchmark\n'+common)
    for i in range(24997):
        out.write(f'route6: 2001:db8:{i:x}::/48\norigin: AS{4200000000+i}\ndescr: Synthetic IPv6 benchmark\n'+common)
with args.output.open('rb') as source:
    checksum = hashlib.file_digest(source, 'sha256').hexdigest()
print(json.dumps({'objects':100000, 'aut_num':50000, 'route':25000, 'route6':24997,
                  'other':3, 'source':'PERF','bytes':args.output.stat().st_size,'sha256':checksum}))
