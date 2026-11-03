#!/usr/bin/env python3
"""Bounded routing snapshot from a public IRRd GraphQL mirror."""
import gzip
import hashlib
import json
from pathlib import Path
import time
import urllib.parse
import urllib.request

MAX_RESPONSE = 128 * 1024 * 1024
MAX_TEXT = 1536 * 1024 * 1024


def query(url, document, variables):
    endpoint = urllib.parse.urlsplit(url)
    if endpoint.scheme != 'https' or not endpoint.hostname or endpoint.username or endpoint.password:
        raise ValueError('GraphQL snapshot requires a credential-free HTTPS endpoint')
    body = json.dumps({'query': document, 'variables': variables}).encode()
    request = urllib.request.Request(url, body, {'Content-Type': 'application/json',
                                                'User-Agent': 'RIRRd-operational-mirror/0.1'})
    started = time.monotonic()
    data = bytearray()
    with urllib.request.urlopen(request, timeout=30) as response:
        if urllib.parse.urlsplit(response.url).scheme != 'https':
            raise ValueError('Insecure GraphQL redirect')
        while block := response.read(65536):
            data.extend(block)
            if len(data) > MAX_RESPONSE or time.monotonic() - started > 300:
                raise ValueError('GraphQL snapshot exceeds byte/time budget')
    value = json.loads(data)
    if value.get('errors') or not isinstance(value.get('data'), dict):
        raise ValueError('GraphQL mirror did not return a complete successful result')
    return value['data'], hashlib.sha256(data).hexdigest(), len(data)


def status(url, source):
    data, _, _ = query(url, 'query($sources:[String!]!){ databaseStatus(sources:$sources){ source serialNewestMirror lastUpdate } }',
                       {'sources': [source]})
    rows = data.get('databaseStatus')
    if not isinstance(rows, list) or len(rows) != 1 or rows[0].get('source') != source:
        raise ValueError('GraphQL mirror source status mismatch')
    serial = rows[0].get('serialNewestMirror')
    if not isinstance(serial, int) or isinstance(serial, bool) or not 0 <= serial <= 2**64 - 1:
        raise ValueError('GraphQL snapshot requires an explicit mirror serial')
    return rows[0]


def fetch(url, source, classes, target):
    before = status(url, source)
    # Include every policy state. A default filtered query is not a mirror snapshot.
    document = '''query($sources:[String!]!,$classes:[String!]!){
      rpslObjects(sources:$sources,objectClass:$classes,recordLimit:2000000,
        rpkiStatus:[valid,invalid,not_found],
        scopeFilterStatus:[in_scope,out_scope_as,out_scope_prefix],
        routePreferenceStatus:[visible,suppressed]) { objectText }
    }'''
    data, response_hash, response_bytes = query(url, document, {'sources': [source], 'classes': classes})
    after = status(url, source)
    if before['serialNewestMirror'] != after['serialNewestMirror'] or before.get('lastUpdate') != after.get('lastUpdate'):
        raise ValueError('GraphQL mirror changed during snapshot; retry before publishing')
    records = data.get('rpslObjects')
    if not isinstance(records, list) or not records or len(records) >= 2000000:
        raise ValueError('GraphQL snapshot is empty, malformed or reaches its record ceiling')
    total = 0
    with Path(target).open('xb') as output:
        with gzip.GzipFile(fileobj=output, mode='wb', mtime=0) as compressed:
            for row in records:
                text = row.get('objectText')
                if not isinstance(text, str):
                    raise ValueError('GraphQL snapshot has missing object text')
                block = (text.rstrip('\n') + '\n\n').encode('utf-8')
                total += len(block)
                if len(block) > 4 * 1024 * 1024 or total > MAX_TEXT:
                    raise ValueError('GraphQL snapshot text exceeds admission bounds')
                compressed.write(block)
    payload = Path(target)
    return {'url': url, 'bytes': payload.stat().st_size, 'sha256': hashlib.sha256(payload.read_bytes()).hexdigest(),
            'transport': 'https-graphql-snapshot', 'transport_authenticated': True,
            'last_modified': None, 'fetched_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
            'response_sha256': response_hash, 'response_bytes': response_bytes,
            'mirror_serial': before['serialNewestMirror'], 'mirror_last_update': before.get('lastUpdate'),
            'records_received': len(records),
            'serialization_note': 'POST GraphQL with all RPKI, scope and preference states; stable peer serial checked before/after; objectText serialized as deterministic gzip. SHA-256 above covers the generated gzip; response hash covers received JSON.'}


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    classes = ['aut-num', 'as-set', 'route-set', 'route', 'route6', 'filter-set', 'peering-set', 'rtr-set', 'inet-rtr']
    receipt = fetch(args.url, args.source, classes, args.output)
    args.output.with_suffix('.receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt))
