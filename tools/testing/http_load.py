#!/usr/bin/env python3
"""Finite native HTTP overload contracts using synthetic data on loopback only.

This is a regression suite, not a published capacity benchmark. It never targets
an operator-supplied host or a public registry. Successful replies are checked in
full, including a multi-megabyte IPv6 result through public admission.
"""
import argparse
from collections import Counter
import concurrent.futures
import http.client
import ipaddress
import json
import os
from pathlib import Path
import threading
import time
import unittest

from harness import Registry
from results import Result, write

LIMIT = 20 * 1024 * 1024
LARGE_COUNT = 65536
QUERY = '/v1/whois/?q=%216AS64496&sources=EXAMPLE'
POLICY = '''http_public_queries = true
[http_public_policy]
rate_per_second = 2000
burst = 2000
max_active = 16
max_response_bytes = 20971520
max_response_buffer_bytes = 67108864
'''


def request(registry, path=QUERY, *, client='192.0.2.1', method='GET', body=None,
            extra=None, timeout=20):
    """Read an entire bounded response; never truncate it to the old 2 MiB cap."""
    connection = http.client.HTTPConnection(*registry.http, timeout=timeout)
    headers = {} if client is None else {'X-RIRRd-Client': client}
    headers.update(extra or {})
    if body is not None:
        headers['Content-Type'] = 'application/json'
    start = time.monotonic()
    try:
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        data = response.read(LIMIT + 1)
        if len(data) > LIMIT:
            raise AssertionError('Response exceeded fixture read bound')
        return response.status, data, dict(response.getheaders()), time.monotonic() - start
    finally:
        connection.close()


class PublicAdmission(unittest.TestCase):
    def setUp(self):
        self.registry = Registry()
        self.addCleanup(self.registry.close)
        with self.registry.config.open('a') as stream:
            stream.write('http_public_queries = true\n')

    def metrics(self):
        status, body, _, _ = request(self.registry, '/ui/http-stats.json', client=None)
        self.assertEqual(status, 200)
        return json.loads(body)

    def test_default_fairness_spoofed_visitor_headers_and_refill(self):
        self.registry.start()
        expected = request(self.registry, client=None)[1]
        outcomes = Counter()
        for index in range(40):
            # Only the proxy-owned header is authoritative. Visitor-controlled
            # forwarding headers cannot gain a new bucket on each request.
            status, body, headers, _ = request(self.registry, extra={
                'X-Forwarded-For': f'198.51.100.{index}',
                'X-Real-IP': f'203.0.113.{index}',
                'Forwarded': f'for=198.51.100.{index}',
            })
            self.assertIn(status, (200, 429))
            outcomes[status] += 1
            if status == 200:
                self.assertEqual(body, expected)
                self.assertIn('server-timing', headers)
            else:
                self.assertEqual(headers.get('retry-after'), '1')
        self.assertGreater(outcomes[200], 0)
        self.assertGreater(outcomes[429], 0)
        before = self.metrics()
        self.assertEqual(before['requests'], 40)
        self.assertEqual(before['rate_per_second'], 2)
        self.assertEqual(before['burst'], 20)
        self.assertEqual(before['max_active'], 16)
        self.assertEqual(before['rate_limited'], outcomes[429])
        self.assertEqual(request(self.registry, client=None)[1], expected)
        self.assertEqual(self.metrics()['requests'], 40, 'Local monitoring stays outside forwarded budget')
        time.sleep(.6)
        self.assertEqual(request(self.registry)[0:2], (200, expected))
        self.assertEqual(self.metrics()['active'], 0)

    def test_proxy_header_syntax_route_allowlist_and_ipv6_subnet_budget(self):
        self.registry.start()
        for header in ['malformed', '192.0.2.1, 198.51.100.1', '']:
            self.assertEqual(request(self.registry, client=header)[0], 400)
        for path in ['/ui/', '/ui/http-stats.json', '/v1/submit/', '/graphql']:
            self.assertEqual(request(self.registry, path)[0], 404)
        wire = self.registry.wire(b'GET /v1/whois/?q=%216AS64496 HTTP/1.1\r\nHost: localhost\r\nX-RIRRd-Client: 192.0.2.1\r\nX-RIRRd-Client: 198.51.100.1\r\nConnection: close\r\n\r\n', http=True)
        self.assertTrue(wire.startswith(b'HTTP/1.1 400'))
        statuses = [request(self.registry, client=f'2001:db8:1::{i + 1:x}')[0] for i in range(40)]
        self.assertGreater(statuses.count(429), 0, 'IPv6 address rotation within /64 must share one budget')
        self.assertEqual(request(self.registry, client='2001:db8:2::1')[0], 200)
        self.assertEqual(request(self.registry, client='::ffff:192.0.2.44')[0], 200)

    def test_invalid_policy_is_rejected_before_database_creation(self):
        invalid = {
            'rate_per_second': [0, 10001, -1, '"fast"', 'nan'],
            'burst': [0, 10001],
            'max_active': [0, 17],
            'max_response_bytes': [0, 1023, 134217729],
            'max_response_buffer_bytes': [0, 134217727, 536870913],
            'unknown_option': [1],
        }
        base = self.registry.config.read_text().replace('registry.redb', 'must-not-exist.redb')
        config = self.registry.root / 'invalid-policy.toml'
        for key, values in invalid.items():
            for value in values:
                with self.subTest(key=key, value=value):
                    config.write_text(base + f'[http_public_policy]\n{key} = {value}\n')
                    result = self.registry.cli('config', 'check', config=config, success=False)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((self.registry.root / 'must-not-exist.redb').exists())

    def test_explicit_response_limit_never_returns_partial_success(self):
        text = self.registry.config.read_text().replace('max_snapshot_bytes = 262144', 'max_snapshot_bytes = 1048576')
        self.registry.config.write_text(text + '[http_public_policy]\nmax_response_bytes = 1024\n')
        fixture = self.registry.root / 'moderate.rpsl'
        fixture.write_text(''.join(f'route6: 2001:db8:{n:x}::/48\norigin: AS64496\nsource: EXAMPLE\n\n' for n in range(128)))
        self.registry.cli('import', 'snapshot', '--source', 'EXAMPLE', '--file', str(fixture))
        self.registry.start()
        status, body, _, _ = request(self.registry)
        self.assertEqual(status, 422)
        self.assertIn(b'narrow the query', body)
        self.assertNotIn(b'2001:db8:', body)
        metrics = self.metrics()
        self.assertEqual(metrics['response_limited'], 1)
        self.assertEqual(metrics['response_errors'], 0)
        self.assertEqual(metrics['active'], 0)
        self.assertEqual(request(self.registry, '/v1/whois/?q=%21v')[0], 200)


class ConcurrentLargeReplies(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.registry = Registry()
        cls.expected_prefixes = sorted(str(ipaddress.IPv6Address(int(ipaddress.IPv6Address('2001:db8:1234:5678:9abc:def0:1234:0')) + n)) + '/128' for n in range(LARGE_COUNT))
        cls.expected = ' '.join(cls.expected_prefixes).encode()
        assert 2 * 1024 * 1024 < len(cls.expected) < LIMIT
        try:
            text = cls.registry.config.read_text().replace('max_snapshot_bytes = 262144', 'max_snapshot_bytes = 16777216').replace('io_timeout_seconds = 2', 'io_timeout_seconds = 15')
            cls.registry.config.write_text(text + POLICY)
            fixture = cls.registry.root / 'large.rpsl'
            with fixture.open('w') as stream:
                for prefix in cls.expected_prefixes:
                    stream.write(f'route6: {prefix}\norigin: AS64496\nsource: EXAMPLE\n\n')
            cls.registry.cli('import', 'snapshot', '--source', 'EXAMPLE', '--file', str(fixture), timeout=180)
            cls.registry.start()
        except BaseException:
            cls.registry.close()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.registry.close()

    def assert_complete(self, reply):
        status, body, headers, _ = reply
        self.assertEqual(status, 200)
        self.assertEqual(len(body), len(self.expected))
        self.assertEqual(body, self.expected)
        self.assertEqual(int(headers['content-length']), len(body))
        self.assertIn('x-rirrd-request-id', headers)

    def test_oversized_legacy_ceiling_now_delivers_complete_whois_and_graphql(self):
        self.assert_complete(request(self.registry))
        status, body, headers, _ = request(self.registry, '/graphql/', method='POST', body=json.dumps({'query':'{ asnPrefixes(asns:[64496], ipVersion:6, sources:["EXAMPLE"]) { asn prefixes } }'}))
        self.assertEqual(status, 200)
        self.assertGreater(len(body), 2 * 1024 * 1024)
        data = json.loads(body)
        self.assertNotIn('errors', data)
        self.assertEqual(data['data']['asnPrefixes'], [{'asn':64496, 'prefixes':self.expected_prefixes}])
        self.assertEqual(int(headers['content-length']), len(body))

    def test_bounded_overload_exact_replies_monitoring_and_recovery(self):
        outcomes = Counter()
        barrier = threading.Barrier(33)
        def load():
            barrier.wait(timeout=10)
            return request(self.registry)
        with concurrent.futures.ThreadPoolExecutor(max_workers=32) as pool:
            futures = [pool.submit(load) for _ in range(32)]
            barrier.wait(timeout=10)
            # Status is served outside admission and the blocking query pool.
            # The loose 2s bound detects queueing behind 32 large query jobs,
            # rather than asserting hardware-specific microbenchmark timings.
            for _ in range(8):
                status, body, _, elapsed = request(self.registry, '/ui/http-stats.json', client=None, timeout=3)
                self.assertEqual(status, 200)
                self.assertLess(elapsed, 2)
                self.assertLessEqual(json.loads(body)['active'], 16)
                time.sleep(.01)
            for future in futures:
                reply = future.result(timeout=25)
                self.assertIn(reply[0], (200, 503), 'No 502, 504, truncated success or transport error is expected')
                outcomes[reply[0]] += 1
                if reply[0] == 200:
                    self.assert_complete(reply)
                else:
                    self.assertEqual(reply[2].get('retry-after'), '1')
        self.assertGreater(outcomes[200], 0)
        # Deterministic semaphore saturation is covered by the Rust test. Fast
        # machines may complete requests before all 32 network clients arrive.
        self.assert_complete(request(self.registry))
        status, body, _, _ = request(self.registry, '/ui/http-stats.json', client=None)
        self.assertEqual(status, 200)
        metrics = json.loads(body)
        self.assertEqual(metrics['active'], 0)
        self.assertEqual(metrics['rate_limited'], 0)
        self.assertEqual(metrics['response_limited'], 0)
        self.assertEqual(metrics['response_errors'], 0)
        self.assertEqual(metrics['deadline_exceeded'], 0)
        self.assertEqual(metrics['rate_per_second'], 2000)
        print(f'Native synthetic overload: {dict(outcomes)}, {LARGE_COUNT} exact prefixes/reply, {len(self.expected)} WHOIS bytes', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    os.environ['RIRRD_TEST_BINARY'] = str(args.binary.resolve())
    suite = unittest.defaultTestLoader.loadTestsFromModule(__import__(__name__))
    result = unittest.TextTestRunner(verbosity=2, resultclass=Result).run(suite)
    write(args.report, getattr(result, 'records', []))
    raise SystemExit(not result.wasSuccessful())
