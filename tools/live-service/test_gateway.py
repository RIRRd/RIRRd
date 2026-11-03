#!/usr/bin/env python3
"""Exercise the actual gateway's allowlist, rate budget and backend framing."""
import http.client
import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest import mock
from types import SimpleNamespace

import gateway
import prepare
import refresh


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

    def test_snapshot_duplicate_canonical_route_uses_last_record(self):
        import gzip
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'input.gz'
            path.write_bytes(gzip.compress(('route: 192.0.2.0/24\norigin: as64496\nremarks: first\nsource: EXAMPLE\n\nroute: 192.000.002.000/24\norigin: AS64496\nremarks: final\u2028paragraph\nsource: EXAMPLE\n').encode('utf-8')))
            output = root / 'normalized'
            counts, excluded, duplicates = prepare.normalize([path], output, 'EXAMPLE', {'route'}, 'utf-8')
            self.assertEqual(counts, {'route': 1})
            self.assertEqual(excluded, {})
            self.assertEqual(duplicates, 1)
            self.assertIn('remarks: final', output.read_text())
            self.assertIn('\u2028paragraph', output.read_text())
            self.assertNotIn('remarks: first', output.read_text())

    def test_plaintext_transport_requires_explicit_official_endpoint(self):
        self.assertEqual(prepare.check_transport('https://example.net/data.gz'), 'https')
        url = 'ftp://ftp.radb.net/radb/dbase/radb.db.gz'
        with self.assertRaises(ValueError):
            prepare.check_transport(url)
        self.assertEqual(prepare.check_transport(url, True), 'ftp')
        for bad in ['http://example.net/data.gz', 'ftp://example.net/data.gz',
                    'ftp://ftp.radb.net:22/radb/dbase/data.gz',
                    'ftp://ftp.radb.net/private/data.gz',
                    'https://user:secret@example.net/data.gz']:
            with self.assertRaises(ValueError):
                prepare.check_transport(bad, True)

    def test_unsafe_member_record_is_withheld_without_inventing_a_name(self):
        import gzip
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            path=root/'input.gz'
            path.write_bytes(gzip.compress(b'as-set: AS-UNSAFE\nmembers: AS-EXA\x1fMPLE\nsource: EXAMPLE\n\naut-num: AS64496\nsource: EXAMPLE\n'))
            rejected={}
            counts,_,_=prepare.normalize([path],root/'output','EXAMPLE',{'as-set','aut-num'},'utf-8',rejected)
            self.assertEqual(counts,{'aut-num':1})
            self.assertEqual(rejected,{'unsafe_ascii_control':1})
            self.assertNotIn('AS-UNSAFE',(root/'output').read_text())

    def test_snapshot_serial_race_is_not_used_as_a_mirror_bootstrap(self):
        import gzip
        import shutil
        binary = Path(__file__).resolve().parents[2] / 'target/release/rust-irrd'
        if not binary.is_file():
            self.skipTest('Build the normal release binary first')
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);catalog=root/'catalog.json';output=root/'candidate'
            catalog.write_text(json.dumps({'classes':['aut-num'], 'refresh':'test', 'sources':[
                {'source':'TEST','state':'enabled','encoding':'utf-8','urls':['https://example.net/test.gz'],
                 'nrtm3':{'host':'example.net','port':43,'interval_seconds':60,'snapshot_serial_url':'https://example.net/TEST.CURRENTSERIAL'}}]}))
            def download(url,target,_allow=False):
                target.write_bytes(gzip.compress(b'aut-num: AS64496\nsource: TEST\n'))
                return {'url':url}
            args=SimpleNamespace(catalog=catalog,binary=binary,output=output,resume=False)
            with mock.patch.object(prepare,'download',download), mock.patch.object(prepare,'snapshot_serial',side_effect=[10,11]), mock.patch.object(prepare.shutil,'disk_usage',return_value=shutil._ntuple_diskusage(100*1024**3,0,100*1024**3)):
                with self.assertRaisesRegex(ValueError,'publication serial changed'):
                    prepare.run(args)
            self.assertFalse((output/'sources.json').exists())

    def test_prepared_activation_rejects_changed_inputs_or_runtime_verification_before_switch(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);tools=root/'tools';tools.mkdir();generations=root/'generations';generations.mkdir()
            previous=generations/'previous';previous.mkdir();candidate=generations/'candidate';candidate.mkdir()
            (root/'current').symlink_to(previous)
            catalog={'sources':[{'source':'TEST','state':'enabled'}]}
            (tools/'sources.json').write_text(json.dumps(catalog))
            digest=hashlib.sha256((tools/'sources.json').read_bytes()).hexdigest()
            report={'binary_sha256':'a'*64,'total_objects':10,'sources':[{'source':'TEST','state':'loaded','objects':10}]}
            (candidate/'sources.json').write_text(json.dumps(report));(candidate/'irrd.toml').write_text('test fixture')
            inputs={'binary_sha256':'a'*64,'catalog_sha256':digest}
            for changed in ['count','catalog']:
                (candidate/'build-inputs.json').write_text(json.dumps({**inputs,'catalog_sha256':'wrong' if changed=='catalog' else digest}))
                with mock.patch.object(refresh,'ROOT',root),mock.patch.object(refresh,'TOOLS',tools),mock.patch.object(refresh,'BINARY','fixture-runtime'),mock.patch.object(refresh.sys,'argv',['refresh.py','--activate','candidate']),mock.patch.object(refresh.subprocess,'run',return_value=SimpleNamespace(stdout='{"objects":9}')) as run:
                    with self.assertRaises(ValueError):refresh.main()
                    self.assertEqual((root/'current').resolve(),previous)
                    self.assertEqual(json.loads((root/'last-refresh.json').read_text())['state'],'failed')
                    self.assertTrue(all(call.args[0][0]=='fixture-runtime' for call in run.call_args_list))
                    self.assertEqual(run.call_count,0 if changed=='catalog' else 2)

    def test_failed_build_resumes_verified_sources_and_rejects_changed_inputs(self):
        import gzip
        import shutil
        binary = Path(__file__).resolve().parents[2] / 'target/release/rust-irrd'
        if not binary.is_file():
            self.skipTest('Build the normal release binary first')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            catalog = root / 'catalog.json'
            catalog.write_text(json.dumps({'classes':['aut-num'], 'refresh':'test', 'sources':[
                {'source':source,'state':'enabled','encoding':'utf-8','urls':['https://example.net/'+source]}
                for source in ['TEST','OTHER']]}))
            output = root / 'candidate'
            args = SimpleNamespace(catalog=catalog, binary=binary, output=output, resume=False)
            calls = []
            def download(url, target, _allow_ftp=False):
                source = url.rsplit('/',1)[1]
                calls.append(source)
                if source=='OTHER' and calls.count('OTHER')==1:
                    raise OSError('simulated upstream failure')
                target.write_bytes(gzip.compress(f'aut-num: AS64496\nsource: {source}\n'.encode()))
                return {'url':url,'sha256':prepare.sha256(target),'bytes':target.stat().st_size}
            with mock.patch.object(prepare, 'download', download), mock.patch.object(prepare.shutil, 'disk_usage', return_value=shutil._ntuple_diskusage(100*1024**3,0,100*1024**3)):
                with self.assertRaises(OSError):
                    prepare.run(args)
                self.assertFalse((output/'sources.json').exists())
                self.assertEqual(set(json.loads((output/'progress.json').read_text())['completed']), {'TEST'})
                args.resume=True
                prepare.run(args)
                self.assertEqual(calls, ['TEST','OTHER','OTHER'])
                self.assertEqual(json.loads((output/'sources.json').read_text())['total_objects'], 2)
                catalog.write_text(catalog.read_text()+'\n')
                with self.assertRaises(ValueError):
                    prepare.run(args)


if __name__ == '__main__':
    unittest.main()
