#!/usr/bin/env python3
"""Compare source precedence and routing answers against the pinned IRRd oracle.

Load query_scope_fixture.py into QUERY-FIRST and QUERY-SECOND in both daemons.
Collections are sorted without removing duplicates; TCP framing is checked exactly.
This is data-result compatibility, not a claim of identical ordering/error prose.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import socket
import sys
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from differential_whois import endpoint, frame, normalise
from query_scope_fixture import SOURCES, records

LIMIT = 256 * 1024


def tcp(address, sources, query, exclude=None):
    commands = ['!!', '!s'+','.join(sources)]
    if exclude is not None:
        commands.append('!e'+','.join(exclude))
    commands += [query, '!q']
    with socket.create_connection(address, timeout=10) as connection:
        connection.settimeout(10)
        connection.sendall(('\n'.join(commands)+'\n').encode())
        raw = bytearray()
        while chunk := connection.recv(8192):
            raw.extend(chunk)
            if len(raw) > LIMIT:
                raise ValueError('Oracle reply exceeds fixture response limit')
    expected = b'C\n'*(2 if exclude is not None else 1)
    if not raw.startswith(expected):
        raise ValueError('Source/exclusion selection failed: '+repr(raw))
    return frame(bytes(raw[len(expected):]))


def http(base, path, document=None):
    data = json.dumps({'query': document}).encode() if document else None
    request = Request(base.rstrip('/')+path, data=data,
                      headers={'Content-Type': 'application/json'} if data else {})
    try:
        response = urlopen(request, timeout=15)
    except HTTPError as error:
        response = error
    with response:
        body = response.read(LIMIT+1)
        if len(body) > LIMIT:
            raise ValueError('Oracle reply exceeds fixture response limit')
        return response.status, body


def object_text(body):
    # Upstream serializes attribute padding and comma-list spacing; RIRRd
    # preserves mirrored value text. Do not normalize arbitrary descriptions.
    body = normalise(body, True, True)
    rows = []
    for block in body.decode().strip().split('\n\n'):
        lines = []
        for line in block.splitlines():
            if line.startswith(('members:', 'mp-members:', 'mnt-by:')):
                line = re.sub(r'\s*,\s*', ',', line)
            lines.append(line)
        rows.append('\n'.join(lines))
    return sorted(rows)


def values(body):
    return sorted(body.decode().split())


def canonical(value):
    if isinstance(value, dict):
        return {key: canonical(item) for key, item in value.items()}
    if isinstance(value, list):
        items = [canonical(item) for item in value]
        return sorted(items, key=lambda item: json.dumps(item, sort_keys=True))
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference-tcp', type=endpoint, default=('127.0.0.1', 8043))
    parser.add_argument('--candidate-tcp', type=endpoint, default=('127.0.0.1', 9043))
    parser.add_argument('--reference-http', default='http://127.0.0.1:8000')
    parser.add_argument('--candidate-http', default='http://127.0.0.1:9080')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    checked = Counter()
    differences = []
    scopes = [list(SOURCES), list(reversed(SOURCES)), [SOURCES[0]], [SOURCES[1]]]

    def equal(group, label, reference, candidate):
        if reference != candidate:
            raise AssertionError(f'{group} {label}: reference={reference!r}; candidate={candidate!r}')
        checked[group] += 1

    def graph(document, expect_error=False):
        replies = []
        statuses = []
        for base in [args.reference_http, args.candidate_http]:
            status, raw = http(base, '/graphql/', document)
            statuses.append(status)
            if status != 200 and not (expect_error and status == 400):
                raise AssertionError((base, status, document))
            value = json.loads(raw)
            if expect_error:
                if not value.get('errors'):
                    raise AssertionError(('Missing GraphQL rejection', base, document))
                replies.append(True)
            else:
                if value.get('errors'):
                    raise AssertionError((base, document, value['errors']))
                replies.append(canonical(value['data']))
        equal('graphql_rejections' if expect_error else 'graphql_results', document, *replies)
        if expect_error and statuses[0] != statuses[1]:
            differences.append({'query': document, 'scope': 'GraphQL rejection HTTP statuses differ',
                                'reference_status': statuses[0], 'candidate_status': statuses[1]})
        return replies[0]

    queries = [
        '!maut-num,AS64496', '!mas-set,AS-DUPLICATE', '!s-lc',
        '!gAS64496', '!6AS64496', '!gAS64497', '!6AS64497',
        '!gAS64498', '!6AS64498', '!gAS64499',
        '!iAS-DUPLICATE', '!iAS-DUPLICATE,1', '!iAS-INDIRECT,1',
        '!iAS-EMPTY', '!iAS-ABSENT,1',
        '!a4AS-DUPLICATE', '!a6AS-DUPLICATE', '!aAS-DUPLICATE',
        '!iRS-QUERY', '!iRS-QUERY,1', '!r192.0.2.0/24,o',
    ]
    for sources in scopes:
        source_list = ','.join(sources)
        for query in queries:
            normalize = object_text if query.startswith('!m') else values
            results = [tcp(address, sources, query) for address in [args.reference_tcp, args.candidate_tcp]]
            equal('tcp_results', source_list+' '+query,
                  (results[0][0], normalize(results[0][1])), (results[1][0], normalize(results[1][1])))
            if query == '!s-lc':
                equal('source_order', source_list, results[0][1].decode().strip(), source_list)
            if query == '!maut-num,AS64496':
                if 'source: '+sources[0] not in object_text(results[0][1])[0]:
                    raise AssertionError('Exact-key query did not use the first selected source')
            # IRRd's HTTP API accepts q only. RIRRd's explicit sources query
            # parameter is an extension, compared with an equivalent TCP session.
            status, body = http(args.candidate_http, '/v1/whois/?'+urlencode({'q': query, 'sources': source_list}))
            expected = 200 if results[0][0] == 'A' else 204
            equal('http_explicit_sources', source_list+' '+query,
                  (expected, normalize(results[0][1])), (status, normalize(body)))

        for query in ['-x 192.0.2.0/24', '-L 192.0.2.1', '-i origin AS64496', 'AS-DUPLICATE']:
            scoped = '-s '+source_list+' '+query
            replies = [http(base, '/v1/whois/?'+urlencode({'q': scoped}))
                       for base in [args.reference_http, args.candidate_http]]
            equal('http_common_requests', scoped,
                  (replies[0][0], object_text(replies[0][1])), (replies[1][0], object_text(replies[1][1])))

        for excluded in [['AS-CHILD'], ['AS-CHILD', 'AS-CYCLE'], ['AS-DUPLICATE']]:
            replies = [tcp(address, sources, '!iAS-DUPLICATE,1', excluded)
                       for address in [args.reference_tcp, args.candidate_tcp]]
            equal('tcp_exclusions', source_list+' '+','.join(excluded),
                  (replies[0][0], values(replies[0][1])), (replies[1][0], values(replies[1][1])))

        source_arg = ',sources:'+json.dumps(sources)
        for family in ['', ',ipVersion:4', ',ipVersion:6']:
            graph('{asnPrefixes(asns:[64496,64497,64498,64499]'+source_arg+family+'){asn prefixes}}')
            graph('{asSetPrefixes(setNames:["AS-DUPLICATE","AS-EMPTY","AS-ABSENT"]'+source_arg+family+'){rpslPk prefixes}}')
        for options in ['', ',depth:1', ',excludeSets:["AS-CHILD"]']:
            graph('{recursiveSetMembers(setNames:["AS-DUPLICATE","AS-ABSENT"]'+source_arg+options+'){rpslPk rootSource members}}')

    for source in SOURCES:
        classes = Counter(record.split(':', 1)[0] for record in records(source))
        if len(classes) != 19 or sum(classes.values()) != 27:
            raise AssertionError('Unexpected full-class oracle fixture inventory')
        for name, count in classes.items():
            document = '{rpslObjects(sources:['+json.dumps(source)+'],objectClass:['+json.dumps(name)+'],mntBy:["QUERY-MNT","OTHER-MNT"]){rpslPk source objectClass}}'
            rows = graph(document)['rpslObjects']
            equal('full_class_inventory', source+' '+name, len(rows), count)

    for query in ['!gASBAD', '!r192.0.2.0/999', '!mroute', '!i', '!a']:
        replies = [tcp(address, [SOURCES[0]], query) for address in [args.reference_tcp, args.candidate_tcp]]
        equal('tcp_rejections', query, [reply[0] for reply in replies], ['F', 'F'])
        if replies[0][1] != replies[1][1]:
            differences.append({'query': query, 'scope': 'Error text differs; both reject with F/HTTP400'})
        replies = [http(base, '/v1/whois/?'+urlencode({'q': query}))
                   for base in [args.reference_http, args.candidate_http]]
        equal('http_rejections', query, [reply[0] for reply in replies], [400, 400])
    for document in [
        '{asnPrefixes(asns:[64496],ipVersion:5){prefixes}}',
    ]:
        graph(document, expect_error=True)

    # These are intentional input-bound differences, not matching oracle cases.
    for document, expected in [
        ('{asnPrefixes(asns:[4294967296]){prefixes}}', {'asnPrefixes': [{'prefixes': []}]}),
        ('{recursiveSetMembers(setNames:["AS-DUPLICATE"],sources:["QUERY-FIRST"],depth:-1){members}}',
         {'recursiveSetMembers': [{'members': ['AS64496', 'AS64497']}]}),
    ]:
        reference_status, raw = http(args.reference_http, '/graphql/', document)
        reference = json.loads(raw)
        candidate_status, raw = http(args.candidate_http, '/graphql/', document)
        candidate = json.loads(raw)
        if reference_status != 200 or reference.get('errors') or canonical(reference.get('data')) != expected:
            raise AssertionError(('Upstream boundary behavior changed', document, reference))
        if candidate_status != 200 or not candidate.get('errors') or candidate.get('data') is not None:
            raise AssertionError(('Candidate accepted out-of-bounds input', document, candidate))
        checked['documented_input_bound_differences'] += 1
        differences.append({'query': document, 'scope': 'RIRRd rejects out-of-bounds input accepted by upstream',
                            'reference_data': reference['data']})

    report = {
        'oracle': 'IRRd 4.5.4 at 8e8080dab744a348b7d0e6efcb53f403301dd075',
        'status': 'passed', 'checks': dict(checked), 'total': sum(checked.values()),
        'fixtures': {source: {'objects': 27, 'classes': 19,
                     'sha256': hashlib.sha256(('\n'.join(records(source))+'\n').encode()).hexdigest()}
                     for source in SOURCES},
        'deliberate_comparison_scope': [
            'TCP A byte lengths and C/D/F framing are validated before comparing values.',
            'Unordered prefix/member/object collections are sorted; source identity is retained.',
            'Object attribute padding, comma-list spacing and known auth placeholder omission are normalized.',
            'RIRRd HTTP sources parameter is an extension compared with the equivalent upstream TCP source selection.',
            'GraphQL rootSource input is a RIRRd extension and is excluded from the shared input corpus.',
            'Malformed requests compare rejection class/status, not upstream error wording.',
            'No live RPKI, scope or route-preference filtering; no full-parity or production-data claim.',
        ],
        'documented_differences': differences,
    }
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
