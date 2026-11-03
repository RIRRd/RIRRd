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
        if self.path == '/v1/rpki/status':
            self.send_response(200);self.end_headers();self.wfile.write(json.dumps({'enabled':True,'usable':True,'rtr_listen':'127.0.0.1:3323','rtr_exposure':'loopback only'}).encode());return
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

    def test_public_rpki_status_does_not_publish_a_localhost_endpoint(self):
        status, body = self.request('GET', '/v1/rpki/status')
        self.assertEqual(status, 200)
        value = json.loads(body)
        self.assertNotIn('rtr_listen', value)
        self.assertNotIn('127.0.0.1', body.decode())
        self.assertIn('HTTPS', value['rtr_exposure'])

    def test_history_is_real_bounded_private_and_survives_restart(self):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'history.json'
            state=gateway.State(Path('/unused'),1,path)
            now=gateway.time.time()
            sample={'total_objects':100,'process_rss_bytes':123456,'cpu_percent_one_core':0.5}
            state.sample(sample,200,now=now-120)
            state.record(.01,False)
            state.sample(sample,201,now=now-60)
            state.sample(sample,202,now=now-59)
            self.assertEqual(len(state.samples()['samples']),2)
            self.assertEqual(state.samples()['samples'][1]['http_qps'],1/60)
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            restored=gateway.State(Path('/unused'),1,path)
            self.assertEqual(len(restored.samples()['samples']),2)
            restored.sample(sample,203,now=now)
            self.assertIsNone(restored.samples()['samples'][-1]['http_qps'])
            restored.sample(sample,204,now=now+86401)
            self.assertEqual(len(restored.samples()['samples']),1)
            self.assertNotIn('prefix',path.read_text())
            self.assertNotIn('client',path.read_text())

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
            runtime=root/'runtime';runtime.write_bytes(b'fixture-runtime')
            (candidate/'registry.redb').write_bytes(b'altered')
            report={'binary_sha256':'a'*64,'total_objects':10,'sources':[{'source':'TEST','state':'loaded','objects':10}]}
            (candidate/'sources.json').write_text(json.dumps(report));(candidate/'irrd.toml').write_text('test fixture')
            inputs={'binary_sha256':'a'*64,'catalog_sha256':digest}
            for changed in ['count','catalog','bytes','config']:
                variant={**report,'runtime_binary_sha256':hashlib.sha256(runtime.read_bytes()).hexdigest(),'database_sha256':hashlib.sha256(b'baseline').hexdigest()} if changed=='bytes' else report
                if changed=='config':variant={**report,'configuration_sha256':'changed'}
                (candidate/'sources.json').write_text(json.dumps(variant))
                (candidate/'build-inputs.json').write_text(json.dumps({**inputs,'catalog_sha256':'wrong' if changed=='catalog' else digest}))
                with mock.patch.object(refresh,'ROOT',root),mock.patch.object(refresh,'TOOLS',tools),mock.patch.object(refresh,'BINARY',str(runtime)),mock.patch.object(refresh.sys,'argv',['refresh.py','--activate','candidate']),mock.patch.object(refresh.subprocess,'run',return_value=SimpleNamespace(stdout='{"objects":9}')) as run:
                    with self.assertRaises(ValueError):refresh.main()
                    self.assertEqual((root/'current').resolve(),previous)
                    self.assertEqual(json.loads((root/'last-refresh.json').read_text())['state'],'failed')
                    self.assertTrue(all(call.args[0][0]==str(runtime) for call in run.call_args_list))
                    self.assertEqual(run.call_count,0 if changed in ['catalog','config'] else 1 if changed=='bytes' else 2)

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
                upgraded=root/'compatible-runtime'
                shutil.copy2(binary,upgraded)
                with upgraded.open('ab') as runtime:runtime.write(b'compatible-test-build-fingerprint')
                args.binary=upgraded
                with self.assertRaises(ValueError):prepare.run(args)
                args.previous_binary=binary
                prepare.run(args)
                report=json.loads((output/'sources.json').read_text())
                self.assertEqual(report['binary_sha256'],prepare.sha256(binary))
                self.assertEqual(report['runtime_binary_sha256'],prepare.sha256(upgraded))
                self.assertEqual(report['database_sha256'],prepare.sha256(output/'registry.redb'))
                catalog.write_text(catalog.read_text()+'\n')
                with self.assertRaises(ValueError):
                    prepare.run(args)

    def test_catalog_extension_reuses_imports_and_rejects_changed_snapshot_inputs(self):
        import gzip
        import shutil
        binary = Path(__file__).resolve().parents[2] / 'target/release/rust-irrd'
        if not binary.exists(): self.skipTest('Build the release binary first')
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);catalog=root/'catalog.json';previous=root/'previous.json';output=root/'candidate'
            source=lambda name:{'source':name,'state':'enabled','encoding':'utf-8','urls':['https://example.net/'+name]}
            data={'classes':['aut-num'],'refresh':'test','sources':[source('TEST')]}
            catalog.write_text(json.dumps(data));calls=[]
            def download(url,target,_allow=False):
                name=url.rsplit('/',1)[1];calls.append(name);target.write_bytes(gzip.compress(f'aut-num: AS64496\nsource: {name}\n'.encode()))
                return {'url':url,'sha256':prepare.sha256(target),'bytes':target.stat().st_size}
            args=SimpleNamespace(catalog=catalog,binary=binary,output=output,resume=False)
            with mock.patch.object(prepare,'download',download),mock.patch.object(prepare.shutil,'disk_usage',return_value=shutil._ntuple_diskusage(100*1024**3,0,100*1024**3)):
                prepare.run(args);shutil.copy2(catalog,previous);args.resume=True;args.previous_catalog=previous
                data['sources'][0]['urls']=['https://example.net/CHANGED'];catalog.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError,'completed snapshot inputs'):prepare.run(args)
                data['sources']=[source('TEST'),source('OTHER')];data['sources'][0]['http_access']=True;catalog.write_text(json.dumps(data))
                prepare.run(args)
                self.assertEqual(calls,['TEST','OTHER'])
                report=json.loads((output/'sources.json').read_text());self.assertEqual(report['total_objects'],2)
                self.assertEqual({r['source'] for r in report['sources']},{'TEST','OTHER'})
                self.assertTrue(list(output.glob('catalog-before-*.json')))
                self.assertEqual(json.loads((output/'build-inputs.json').read_text())['catalog_sha256'],prepare.sha256(catalog))

    def test_graphql_snapshot_includes_all_policy_states_and_rejects_serial_races(self):
        import gzip
        import graphql_snapshot
        before={'databaseStatus':[{'source':'TEST','serialNewestMirror':10,'lastUpdate':'test'}]}
        records={'rpslObjects':[{'objectText':'aut-num: AS64496\nsource: TEST\n'}]}
        with tempfile.TemporaryDirectory() as directory:
            target=Path(directory)/'source.gz'
            with mock.patch.object(graphql_snapshot,'query',side_effect=[(before,'s',1),(records,'payload',42),(before,'s',1)]) as query:
                receipt=graphql_snapshot.fetch('https://example.net/graphql/','TEST',['aut-num'],target)
                self.assertEqual(receipt['mirror_serial'],10)
                self.assertIn(b'source: TEST',gzip.decompress(target.read_bytes()))
                document=query.call_args_list[1].args[1]
                for expected in ['rpkiStatus:[valid,invalid,not_found]','scopeFilterStatus:[in_scope,out_scope_as,out_scope_prefix]','routePreferenceStatus:[visible,suppressed]']:
                    self.assertIn(expected,document)
            target.unlink();changed={'databaseStatus':[{'source':'TEST','serialNewestMirror':11,'lastUpdate':'test'}]}
            with mock.patch.object(graphql_snapshot,'query',side_effect=[(before,'s',1),(records,'payload',42),(changed,'s',1)]):
                with self.assertRaisesRegex(ValueError,'changed during snapshot'):graphql_snapshot.fetch('https://example.net/graphql/','TEST',['aut-num'],target)
            self.assertFalse(target.exists())
            with self.assertRaises(ValueError):graphql_snapshot.query('http://example.net/','{}',{})

    def test_atomic_runtime_upgrade_rolls_back_without_copying_the_database(self):
        import os
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);tools=root/'tools';tools.mkdir();generations=root/'generations';generations.mkdir()
            previous=generations/'old';previous.mkdir();candidate=generations/'new';candidate.mkdir();(root/'current').symlink_to(previous)
            old_runtime=root/'installed';old_runtime.write_bytes(b'old runtime');old_runtime.chmod(0o755)
            replacement=root/'replacement';replacement.write_bytes(b'new runtime');replacement.chmod(0o755)
            old_data=previous/'registry.redb';old_data.write_bytes(b'old database')
            database=candidate/'registry.redb';database.write_bytes(b'new database')
            config=candidate/'irrd.toml';config.write_text('test configuration')
            catalog={'sources':[{'source':'TEST','state':'enabled'}]};(tools/'sources.json').write_text(json.dumps(catalog))
            digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
            report={'sources':[{'source':'TEST','state':'loaded','objects':1}], 'total_objects':1,'binary_sha256':'build',
                    'runtime_binary_sha256':digest(replacement),'database_sha256':digest(database),'configuration_sha256':digest(config),'verified_at':'test'}
            (candidate/'sources.json').write_text(json.dumps(report));(previous/'sources.json').write_text(json.dumps(report))
            (candidate/'build-inputs.json').write_text(json.dumps({'catalog_sha256':digest(tools/'sources.json'),'binary_sha256':'build'}))
            with mock.patch.object(refresh,'ROOT',root),mock.patch.object(refresh,'TOOLS',tools),mock.patch.object(refresh,'BINARY',str(old_runtime)),mock.patch('sys.argv',['refresh','--activate','new','--runtime',str(replacement)]),mock.patch('pwd.getpwnam',return_value=SimpleNamespace(pw_uid=os.getuid(),pw_gid=os.getgid())),mock.patch.object(refresh.subprocess,'run') as run,mock.patch.object(refresh,'healthy',side_effect=RuntimeError('simulated failed health')):
                with self.assertRaisesRegex(RuntimeError,'failed health'):refresh.main()
                service_calls=[call.args[0] for call in run.call_args_list if call.args[0][0]=='systemctl']
                self.assertEqual([c[1] for c in service_calls],['stop','start','stop','start'])
            self.assertEqual((root/'current').resolve(),previous)
            self.assertEqual(old_runtime.read_bytes(),b'old runtime')
            self.assertEqual(old_data.read_bytes(),b'old database')
            self.assertEqual(database.read_bytes(),b'new database')


if __name__ == '__main__':
    unittest.main()
