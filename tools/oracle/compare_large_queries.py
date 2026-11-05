#!/usr/bin/env python3
"""Compare complete large prefix/set answers; never accept partial success."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys
from time import monotonic
from urllib.parse import urlencode

import compare_query_scope as query
from large_query_fixture import (
    ASNS_PER_CHILD, CHILDREN, FIRST_ASN, INDIRECT_MEMBERS, ORIGINS,
    SINGLE_ASN, SOURCE, child_name, prefix, records,
)

# Fixtures are bounded independently of production daemon limits. The largest
# case exceeds the former 256 KiB response budget and still fits below 1 MiB.
query.LIMIT = 2 * 1024 * 1024


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference-tcp', type=query.endpoint, default=('127.0.0.1', 8043))
    parser.add_argument('--candidate-tcp', type=query.endpoint, default=('127.0.0.1', 9043))
    parser.add_argument('--reference-http', default='http://127.0.0.1:8000')
    parser.add_argument('--candidate-http', default='http://127.0.0.1:9080')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    expected_members = sorted(f'AS{FIRST_ASN + i}' for i in range(ORIGINS))
    v4 = sorted(prefix(i, 4) for i in range(ORIGINS * 2))
    v6 = sorted(prefix(i, 6) for i in range(ORIGINS))
    single4 = sorted(prefix(i, 4, True) for i in range(ORIGINS))
    single6 = sorted(prefix(i, 6, True) for i in range(ORIGINS))
    indirect = sorted(f'AS{FIRST_ASN + i}' for i in range(INDIRECT_MEMBERS))
    cases = [
        ('!iAS-LARGE', sorted(child_name(i) for i in range(CHILDREN))),
        ('!iAS-LARGE,1', expected_members),
        ('!a4AS-LARGE', v4), ('!a6AS-LARGE', v6), ('!aAS-LARGE', sorted(v4+v6)),
        (f'!gAS{SINGLE_ASN}', single4), (f'!6AS{SINGLE_ASN}', single6),
        ('!a4AS-LARGE-ORIGIN', single4), ('!a6AS-LARGE-ORIGIN', single6),
        ('!iAS-LARGE-INDIRECT,1', indirect),
        ('!a4AS-LARGE-INDIRECT', sorted(prefix(i, 4) for i in range(INDIRECT_MEMBERS * 2))),
        ('!a6AS-LARGE-INDIRECT', sorted(prefix(i, 6) for i in range(INDIRECT_MEMBERS))),
        ('!iAS-LARGE-MISSING,1', []),
    ]
    checks, failures, measurements = Counter(), [], []

    def check(transport, label, expected, operation):
        started = monotonic()
        try:
            actual = operation()
            if actual != expected:
                raise AssertionError('complete response differs from independently generated expected answer')
            checks[transport] += 1
        except (AssertionError, ValueError, OSError) as error:
            failures.append({'transport': transport, 'query': label, 'error': str(error)[:500]})
        measurements.append({'transport': transport, 'query': label,
                             'elapsed_seconds': round(monotonic() - started, 6)})

    def tcp(address, command, expected, excluded=None):
        status, body = query.tcp(address, [SOURCE], command, excluded)
        if status != ('A' if expected else 'D'):
            raise AssertionError(f'expected complete answer, received {status}: '+body[:200].decode())
        return query.values(body)

    def http(command, expected):
        status, body = query.http(args.candidate_http, '/v1/whois/?'+urlencode({'q': command, 'sources': SOURCE}))
        if status != (200 if expected else 204):
            raise AssertionError(f'expected complete HTTP answer, received {status}: '+body[:200].decode())
        return query.values(body)

    for command, expected in cases:
        check('reference_tcp', command, expected, lambda: tcp(args.reference_tcp, command, expected))
        check('candidate_tcp', command, expected, lambda: tcp(args.candidate_tcp, command, expected))
        check('candidate_http', command, expected, lambda: http(command, expected))

    excluded = [child_name(0)]
    remaining = sorted(f'AS{FIRST_ASN+i}' for i in range(ASNS_PER_CHILD, ORIGINS))
    for name, address in [('reference_tcp', args.reference_tcp), ('candidate_tcp', args.candidate_tcp)]:
        check(name, '!iAS-LARGE,1 excluding first child', remaining,
              lambda: tcp(address, '!iAS-LARGE,1', remaining, excluded))

    def graph(base, document):
        status, raw = query.http(base, '/graphql/', document)
        value = json.loads(raw)
        if status != 200 or value.get('errors'):
            raise AssertionError(f'expected complete GraphQL answer, received {status}: '+str(value.get('errors'))[:200])
        return query.canonical(value.get('data'))

    documents = []
    for version, prefixes in [(4, v4), (6, v6), (None, v4+v6)]:
        family = f',ipVersion:{version}' if version else ''
        document = '{asSetPrefixes(setNames:["AS-LARGE"],sources:["LARGE"]'+family+'){rpslPk prefixes}}'
        documents.append((document, {'asSetPrefixes': [{'rpslPk': 'AS-LARGE', 'prefixes': prefixes}]}))
        single = single4 if version == 4 else single6 if version == 6 else single4+single6
        document = '{asnPrefixes(asns:['+str(SINGLE_ASN)+'],sources:["LARGE"]'+family+'){asn prefixes}}'
        documents.append((document, {'asnPrefixes': [{'asn': SINGLE_ASN, 'prefixes': single}]}))
    for name, members in [('AS-LARGE', expected_members), ('AS-LARGE-INDIRECT', indirect)]:
        document = '{recursiveSetMembers(setNames:['+json.dumps(name)+'],sources:["LARGE"]){rpslPk rootSource members}}'
        documents.append((document, {'recursiveSetMembers': [{'rpslPk': name, 'rootSource': SOURCE, 'members': members}]}))
    document = '{recursiveSetMembers(setNames:["AS-LARGE"],sources:["LARGE"],excludeSets:['+json.dumps(child_name(0))+']){members}}'
    documents.append((document, {'recursiveSetMembers': [{'members': remaining}]}))
    for document, expected in documents:
        for name, base in [('reference_graphql', args.reference_http), ('candidate_graphql', args.candidate_http)]:
            check(name, document, query.canonical(expected), lambda: graph(base, document))

    # Deliberate finite-budget behavior, not an upstream-equivalence assertion:
    # each alias fits the 262144-result budget, but together they exceed it.
    asns = ','.join([str(SINGLE_ASN)] * 32)
    field = 'asnPrefixes(asns:['+asns+'],sources:["LARGE"],ipVersion:4){prefixes}'
    document = '{first:'+field+' second:'+field+'}'

    def reject_aggregate():
        status, raw = query.http(args.candidate_http, '/graphql/', document)
        value = json.loads(raw)
        if status != 200 or value.get('data') is not None:
            return False
        return any('limit' in error.get('message', '').lower() for error in value.get('errors', []))

    check('candidate_result_budget', 'two aliases exceed aggregate prefix budget', True, reject_aggregate)
    check('candidate_recovery', f'!gAS{SINGLE_ASN} after aggregate rejection', single4,
          lambda: http(f'!gAS{SINGLE_ASN}', single4))

    fixture_hash = hashlib.sha256()
    fixture_classes = Counter()
    for record in records():
        fixture_hash.update(record.encode())
        fixture_classes[record.split(':', 1)[0]] += 1
    report = {
        'oracle': 'IRRd 4.5.4 at 8e8080dab744a348b7d0e6efcb53f403301dd075',
        'status': 'failed' if failures else 'passed', 'checks': dict(checks),
        'total_passed': sum(checks.values()), 'failures': failures,
        'fixture': {'source': SOURCE, 'objects': sum(fixture_classes.values()),
                    'classes': dict(fixture_classes), 'sha256': fixture_hash.hexdigest(),
                    'child_sets': CHILDREN, 'origins': ORIGINS,
                    'root_ipv4_prefixes': len(v4), 'root_ipv6_prefixes': len(v6),
                    'single_origin_prefixes_per_family': ORIGINS,
                    'authorized_indirect_members': INDIRECT_MEMBERS},
        'comparison_scope': '59 complete-answer checks with duplicates retained and exact TCP framing; 2 candidate-only aggregate-budget/recovery checks. Synthetic unfiltered source only.',
        'timings': measurements,
        'timing_scope': 'Serial correctness checks, including connection setup; not a throughput or sustained-load benchmark.',
    }
    encoded = json.dumps(report, indent=2)+'\n'
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded)
    print(encoded)
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
