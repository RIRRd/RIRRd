#!/usr/bin/env python3
"""Exercise the actual gateway's allowlist, rate budget and backend framing."""
import http.client
import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest

import gateway
import prepare


class Backend(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'aut-num: AS64496\nsource: EXAMPLE\n')

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps({'data': json.loads(data)}).encode())

    def log_message(self, *_args):
        pass


class GatewayChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.backend = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Backend)
        cls.state = gateway.State(Path('/nonexistent'), cls.backend.server_port)
        cls.server = gateway.Server(('127.0.0.1', 0), cls.state)
        for server in (cls.backend, cls.server):
            threading.Thread(target=server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        for server in (cls.server, cls.backend):
            server.shutdown()
            server.server_close()

    def request(self, method, path, body=None, headers=None):
        connection = http.client.HTTPConnection('127.0.0.1', self.server.server_port, timeout=3)
        connection.request(method, path, body, headers or {})
        response = connection.getresponse()
        status, data = response.status, response.read()
        connection.close()
        return status, data

    def test_read_only_routes_and_telemetry(self):
        for method, path in [('POST', '/v1/submit/'), ('GET', '/admin'),
                             ('POST', '/graphql'), ('GET', '/graphql/'),
                             ('POST', '/dashboard/meta.json'), ('GET', '/v1/whois/extra')]:
            self.assertEqual(self.request(method, path)[0], 404)
        status, data = self.request('GET', '/v1/whois/?q=AS64496')
        self.assertEqual(status, 200)
        self.assertIn(b'AS64496', data)
        status, data = self.request('POST', '/graphql/', '{"query":"{ databaseStatus { source } }"}')
        self.assertEqual(status, 200)
        self.assertIn('query', json.loads(data)['data'])
        status, data = self.request('GET', '/dashboard/live.json')
        self.assertEqual(status, 200)
        self.assertGreaterEqual(json.loads(data)['http_queries'], 2)
        self.assertNotIn('generation', data.decode())

    def test_bounds_and_per_client_rate(self):
        self.assertEqual(self.request('POST', '/graphql/', b'X' * (gateway.MAX_BODY + 1))[0], 413)
        self.assertEqual(self.request('GET', '/v1/whois/?q=A', headers={'Transfer-Encoding': 'chunked'})[0], 400)
        state = gateway.State(Path('/unused'), 1)
        self.assertTrue(all(state.allow('192.0.2.1', 10) for _ in range(20)))
        self.assertFalse(state.allow('192.0.2.1', 10))
        self.assertTrue(state.allow('192.0.2.1', 10.5))
        self.assertTrue(all(state.allow('2001:db8::1', 10) for _ in range(20)))
        self.assertFalse(state.allow('2001:db8::ffff', 10))
        self.assertTrue(state.allow('2001:db8:1::1', 10))

    def test_snapshot_source_mismatch_and_credentials_fail_closed(self):
        import gzip
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'input.gz'
            for index, text in enumerate(['route: 192.0.2.0/24\norigin: AS64496\nsource: WRONG\n',
                                           'route: 192.0.2.0/24\norigin: AS64496\nauth: DUMMY\nsource: EXAMPLE\n']):
                path.write_bytes(gzip.compress(text.encode()))
                with self.assertRaises(ValueError):
                    prepare.normalize([path], root / f'output-{index}', 'EXAMPLE', {'route'}, 'utf-8')


if __name__ == '__main__':
    unittest.main()
