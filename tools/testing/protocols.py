#!/usr/bin/env python3
"""Independent black-box contracts against a private synthetic RIRRd process."""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import unittest
from urllib.parse import urlencode

from harness import Registry, payload
from results import Result, write


class Protocols(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.registry = Registry()
        try:
            cls.registry.start()
        except BaseException:
            cls.registry.close()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.registry.close()

    def lookup(self, query, sources='EXAMPLE'):
        parameters = {'q':query}
        if sources is not None:
            parameters['sources'] = sources
        return self.registry.request('/v1/whois/?'+urlencode(parameters))

    def graphql(self, query):
        status, raw, _ = self.registry.request('/graphql/', 'POST', json.dumps({'query':query}), {'Content-Type':'application/json'})
        self.assertEqual(status, 200)
        return json.loads(raw)

    def test_offline_repack_retains_class_counts_and_public_queries(self):
        registry = Registry()
        try:
            before = json.loads(registry.cli('database', 'count', '--by-class').stdout)
            self.assertFalse(before['integrity_verified'])
            self.assertEqual(sum(sum(classes.values()) for classes in before['class_counts'].values()), before['objects'])
            plain = json.loads(registry.cli('database', 'count').stdout)
            self.assertEqual(set(plain), {'objects', 'integrity_verified'})
            packed = registry.root / 'packed.redb'
            result = json.loads(registry.cli('database', 'repack', '--file', str(packed)).stdout)
            self.assertEqual(result['objects'], before['objects'])
            self.assertEqual(len(result['logical_sha256']), 64)
            self.assertNotEqual(registry.cli('database', 'repack', '--file', str(packed), success=False).returncode, 0)
            registry.config.write_text(registry.config.read_text().replace('"registry.redb"', '"packed.redb"'))
            self.assertEqual(json.loads(registry.cli('database', 'count', '--by-class').stdout), before)
            registry.start()
            self.assertIn(b'192.0.2.0/24', payload(registry.wire(b'!gAS64496\n')))
            status, raw, _ = registry.request('/v1/whois/?q=%216AS64496')
            self.assertEqual(status, 200)
            self.assertIn(b'2001:db8:100::/48', raw)
        finally:
            registry.close()

    def test_tcp_utf8_length_is_bytes(self):
        raw = self.registry.wire(b'!!\n!sSECOND\n!maut-num,AS64496\n!q\n')
        self.assertTrue(raw.startswith(b'C\n'))
        self.assertIn('café'.encode(), payload(raw[2:]))

    def test_tcp_sessions_keep_source_selection_private(self):
        raw = self.registry.wire(b'!!\n!sSECOND\n!maut-num,AS64496\n!q\n')
        self.assertIn(b'SECONDARY', payload(raw[2:]))
        self.assertNotIn(b'SECONDARY', payload(self.registry.wire(b'!maut-num,AS64496\n')))

    def test_tcp_pipeline_exact_frames(self):
        raw = self.registry.wire(b'!!\r\n!gAS64496\r\n!6AS64496\r\n!q\r\n')
        # !! has no reply; each query must have its own byte-exact frame.
        first, _, rest = raw.partition(b'\n')
        size = int(first[1:]); frame = first+b'\n'+rest[:size+2]
        self.assertIn(b'192.0.2.0/24', payload(frame))
        self.assertEqual(payload(rest[size+2:]), b'2001:db8:100::/48\n')

    def test_parallel_clients_do_not_change_each_others_sources(self):
        def lookup(n):
            source = 'EXAMPLE' if n%2 else 'SECOND'
            status, raw, _ = self.lookup('!maut-num,AS64496', source)
            self.assertEqual(status, 200)
            self.assertIn(('source: '+source).encode(), raw)
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(lookup, range(24)))

    def test_graphql_redacts_authentication(self):
        data = self.graphql('{ rpslObjects(rpslPk:["DOCS-MNT"]) { rpslPk objectText } }')
        text = data['data']['rpslObjects'][0]['objectText']
        self.assertIn('DOCS-MNT', text)
        self.assertNotIn('auth:', text.lower())
        self.assertNotIn('DummyValue', text)

    def test_graphql_introspection_has_real_query_fields(self):
        data = self.graphql('{ __schema { queryType { fields { name } } } }')
        names = {f['name'] for f in data['data']['__schema']['queryType']['fields']}
        self.assertTrue({'rpslObjects','databaseStatus','asnPrefixes','asSetPrefixes','recursiveSetMembers'} <= names)

    def test_graphql_unknown_source_is_an_error(self):
        self.assertTrue(self.graphql('{ rpslObjects(sources:["ABSENT"]) { rpslPk } }').get('errors'))

    def test_graphql_unknown_field_is_an_error(self):
        self.assertTrue(self.graphql('{ unsupportedParityClaim }').get('errors'))

    def test_graphql_read_only_schema_rejects_mutation(self):
        self.assertTrue(self.graphql('mutation { deleteEverything }').get('errors'))
        self.assertEqual(self.lookup('!gAS64496')[0], 200)

    def test_graphql_syntax_error_does_not_stop_server(self):
        self.assertTrue(self.graphql('{ rpslObjects(').get('errors'))
        self.assertEqual(self.lookup('!gAS64496')[0], 200)

    def test_graphql_source_precedence(self):
        data = self.graphql('{ rpslObjects(rpslPk:["AS64496"],sources:["SECOND"]) { source objectText } }')
        self.assertEqual(data['data']['rpslObjects'][0]['source'], 'SECOND')

    def test_key_pagination_has_no_duplicate_or_missing_keys(self):
        path = '/v1/irr/objects?source=EXAMPLE&class=as-set&limit=1'
        status, raw, _ = self.registry.request(path)
        self.assertEqual(status, 200); first = json.loads(raw)
        self.assertEqual(len(first['keys']), 1)
        status, raw, _ = self.registry.request(path+'&'+urlencode({'after':first['next_cursor']}))
        self.assertEqual(status, 200); second = json.loads(raw)
        self.assertEqual(len(second['keys']), 1)
        self.assertIsNone(second['next_cursor'])
        self.assertEqual(set(first['keys']+second['keys']), {'AS64496:AS-DOCS','AS64496:AS-CHILD'})

    def test_retired_validator_endpoints_are_absent(self):
        for path in ['/v1/rpki/status', '/v1/rpki/validate?prefix=192.0.2.0/24&origin=AS64496']:
            self.assertEqual(self.registry.request(path)[0], 404)
        self.assertEqual(self.registry.request('/v1/rpki/validate', 'POST', '{}', {'Content-Type':'application/json'})[0], 404)

    def test_disabled_submission_has_no_write_endpoint(self):
        self.assertEqual(self.registry.request('/v1/submit/', 'POST', '{}', {'Content-Type':'application/json'})[0], 404)
        self.assertEqual(self.lookup('!gAS64496')[0], 200)

    def test_bad_host_is_rejected_before_every_surface(self):
        for path in ['/graphql/','/v1/whois/?q=AS64496','/ui/meta.json','/v1/irr/objects?source=EXAMPLE&class=route','/v1/rpki/status']:
            with self.subTest(path=path):
                self.assertEqual(self.registry.request(path, headers={'Host':'untrusted.example.invalid'})[0], 400)

    def test_duplicate_host_rejected(self):
        raw = self.registry.wire(b'GET /ui/meta.json HTTP/1.1\r\nHost: localhost\r\nHost: evil.example.invalid\r\nConnection: close\r\n\r\n', http=True)
        self.assertTrue(raw.startswith(b'HTTP/1.1 400 '))

    def test_source_selection_is_stateless_over_http(self):
        self.assertIn(b'SECONDARY', self.lookup('!maut-num,AS64496', 'SECOND')[1])
        self.assertNotIn(b'SECONDARY', self.lookup('!maut-num,AS64496', None)[1])


def tcp_case(query, expected):
    def test(self):
        raw = self.registry.wire(query.encode()+b'\n')
        if expected is None:
            self.assertEqual(raw, b'D\n')
        elif expected is False:
            self.assertTrue(raw.startswith(b'F'), raw)
        else:
            body = payload(raw)
            self.assertIn(expected.encode(), body)
            self.assertNotIn(b'DummyValue', body)
            self.assertNotIn(b'auth:', body)
    return test


for name, query, expected in [
    ('exact_asn','!maut-num,AS64496','DOCS-ONE'),
    ('case_folded_asn','!maut-num,as64496','DOCS-ONE'),
    ('v4_origin','!gAS64497','198.51.100.0/24'),
    ('v6_origin','!6AS64497','2001:db8:200::/48'),
    ('v4_exact','!r192.0.2.0/24','192.0.2.0/24'),
    ('v6_exact','!r2001:db8:100::/48','2001:db8:100::/48'),
    ('v4_no_false_cover','!r192.0.2.0/25',None),
    ('v6_no_false_cover','!r2001:db8:100:1::/64',None),
    ('v4_strict_cover','!r192.0.2.0/25,l','192.0.2.0/24'),
    ('v6_strict_cover','!r2001:db8:100:1::/64,l','2001:db8:100::/48'),
    ('v4_more_specific','!r192.0.2.0/23,M','192.0.2.0/24'),
    ('v6_more_specific','!r2001:db8::/32,M','2001:db8:100::/48'),
    ('recursive_members','!iAS64496:AS-DOCS,1','AS64497'),
    ('recursive_v4','!a4AS64496:AS-DOCS','198.51.100.0/24'),
    ('recursive_v6','!a6AS64496:AS-DOCS','2001:db8:200::/48'),
    ('redacted_maintainer','!mmntner,DOCS-MNT','DOCS-MNT'),
    ('absent_object','!maut-num,AS64511',None),
    ('absent_origin','!gAS64511',None),
    ('malformed_prefix','!r192.0.2.0/999',False),
    ('unknown_command','!unsupported',False),
]:
    setattr(Protocols, 'test_tcp_'+name, tcp_case(query, expected))


def http_case(path, status, contains=None):
    def test(self):
        actual, raw, _ = self.registry.request(path)
        self.assertEqual(actual, status, raw[:200])
        if contains:
            self.assertIn(contains.encode(), raw)
        if status==204:
            self.assertEqual(raw, b'')
        self.assertNotIn(b'DummyValue', raw)
    return test


for name, path, status, contains in [
    ('origin_v4','/v1/whois/?q=%21gAS64496&sources=EXAMPLE',200,'192.0.2.0/24'),
    ('origin_v6','/v1/whois/?q=%216AS64496&sources=EXAMPLE',200,'2001:db8:100::/48'),
    ('absent','/v1/whois/?q=%21gAS64511',204,None),
    ('missing_query','/v1/whois/',400,None),
    ('unknown_source','/v1/whois/?q=AS64496&sources=ABSENT',400,None),
    ('null_query','/v1/whois/?q=%00',400,None),
    ('newline_query','/v1/whois/?q=AS64496%0A%21gAS64497',400,None),
    ('oversized_query','/v1/whois/?q='+('A'*5000),400,None),
    ('unknown_browse_source','/v1/irr/objects?source=ABSENT&class=route',400,None),
    ('maintainer_keys_redacted','/v1/irr/objects?source=EXAMPLE&class=mntner',200,'DOCS-MNT'),
    ('oversized_page','/v1/irr/objects?source=EXAMPLE&class=route&limit=101',400,None),
    ('bad_cursor','/v1/irr/objects?source=EXAMPLE&class=route&after=%00',400,None),
    ('console','/ui/',200,'Registry explorer'),
    ('metadata','/ui/meta.json',200,'EXAMPLE'),
    ('unrelated_path','/not-an-api',404,None),
]:
    setattr(Protocols, 'test_http_'+name, http_case(path, status, contains))


class Recovery(unittest.TestCase):
    def setUp(self):
        self.registry = Registry()
        self.addCleanup(self.registry.close)

    def test_invalid_import_preserves_committed_source(self):
        file = self.registry.root/'bad.rpsl'; file.write_text('route: malformed\nsource: EXAMPLE\n')
        self.assertNotEqual(self.registry.cli('import','snapshot','--source','EXAMPLE','--file',str(file),success=False).returncode,0)
        self.registry.cli('database','verify')
        self.registry.start()
        self.assertIn(b'192.0.2.0/24', payload(self.registry.wire(b'!gAS64496\n')))

    def test_invalid_config_does_not_create_database(self):
        config = self.registry.root/'invalid.toml';config.write_text('database="must-not-exist.redb"\nsources=["EXAMPLE"]\nlisten="127.0.0.1:0"\ndatabase_cache_mib=0\n')
        self.assertNotEqual(self.registry.cli('config','check',config=config,success=False).returncode,0)
        self.assertFalse((self.registry.root/'must-not-exist.redb').exists())

    def test_restart_reopens_the_same_committed_database(self):
        self.registry.start(); first=payload(self.registry.wire(b'!6AS64496\n'));self.registry.stop()
        self.registry.cli('database','verify');self.registry.start()
        self.assertEqual(payload(self.registry.wire(b'!6AS64496\n')),first)

    def test_backup_restore_and_checksum_rejection(self):
        archive=self.registry.root/'registry.backup'
        self.registry.cli('database','backup','--file',str(archive))
        config=self.registry.root/'restore.toml';config.write_text(self.registry.config.read_text().replace('registry.redb','restored.redb'))
        self.registry.cli('database','restore','--file',str(archive),config=config)
        self.registry.cli('database','verify',config=config)
        # A second destination must not be silently overwritten.
        self.assertNotEqual(self.registry.cli('database','restore','--file',str(archive),config=config,success=False).returncode,0)
        damaged=bytearray(archive.read_bytes());damaged[-1]^=1;archive.write_bytes(damaged)
        config.write_text(config.read_text().replace('restored.redb','damaged.redb'))
        self.assertNotEqual(self.registry.cli('database','restore','--file',str(archive),config=config,success=False).returncode,0)
        self.assertFalse((self.registry.root/'damaged.redb').exists())


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True);parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args();os.environ['RIRRD_TEST_BINARY']=str(args.binary.resolve())
    suite=unittest.defaultTestLoader.loadTestsFromModule(__import__(__name__))
    result=unittest.TextTestRunner(verbosity=2,resultclass=Result).run(suite)
    write(args.report,getattr(result,'records',[]))
    raise SystemExit(not result.wasSuccessful())
