#!/usr/bin/env python3
import hashlib
import http.client
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('community_service', Path(__file__).with_name('service.py'))
service = importlib.util.module_from_spec(spec)
spec.loader.exec_module(service)


def submission():
    return {'name':'Example UK node','operator':'Example operator','contact':'operator@example.net',
            'country':'United Kingdom','country_code':'GB','region':'Europe','provider':'Example host',
            'sponsor':'Example sponsor','whois_host':'irr.example.net','whois_port':43,
            'https':'https://irr.example.net/','notes':'Private review note','authorised':True,'website':''}


class CommunityTests(unittest.TestCase):
    def test_validates_public_endpoints_without_fetching_them(self):
        self.assertEqual(service.validate(submission())['whois_host'],'irr.example.net')
        for field,value in [('whois_host','127.0.0.1'),('whois_host','::1'),('whois_host','10.0.0.1'),('whois_host','localhost'),('https','http://irr.example.net/'),('https','https://user:password@irr.example.net/'),('https','https://[::1]/'),('whois_port',True),('authorised',False),('website','bot-filled'),('contact','not-an-email')]:
            data=submission();data[field]=value
            with self.assertRaises(ValueError,msg=(field,value)):service.validate(data)
        data=submission();data['notes']='Multiple\nreview lines';self.assertIn('\n',service.validate(data)['notes'])
        with mock.patch.object(service,'public_host',wraps=service.public_host):
            service.validate(submission())  # Validation performs no URL requests or DNS lookups.

    def test_durable_queue_rate_limit_private_storage_and_retention(self):
        with tempfile.TemporaryDirectory() as root:
            queue=service.Queue(root)
            ids=[queue.submit(submission(),'2001:db8::1',now=10000000) for _ in range(3)]
            self.assertEqual(len(set(ids)),3)
            with self.assertRaises(OverflowError):queue.submit(submission(),'2001:db8::ffff',now=10000001)
            with queue.connect() as db:
                rows=db.execute('SELECT client,payload FROM submissions').fetchall()
                self.assertNotIn('2001:db8',rows[0][0]);self.assertEqual(len(rows[0][0]),64)
                self.assertIn('operator@example.net',rows[0][1])
            self.assertEqual(queue.database.stat().st_mode & 0o777,0o600)
            self.assertEqual(Path(root).stat().st_mode & 0o777,0o700)
            queue.submit(submission(),'192.0.2.1',now=10000000+91*86400)
            with queue.connect() as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM submissions').fetchone()[0],1)

    def test_http_submission_is_private_origin_checked_and_bounded(self):
        with tempfile.TemporaryDirectory() as root:
            queue=service.Queue(root);server=service.Server(('127.0.0.1',0),queue,'https://community.example')
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            def request(method,path,data=None,headers=None):
                connection=http.client.HTTPConnection(*server.server_address,timeout=3)
                body=json.dumps(data) if isinstance(data,dict) else data
                connection.request(method,path,body,{'Content-Type':'application/json','Origin':'https://community.example',**(headers or {})})
                response=connection.getresponse();result=(response.status,response.read());connection.close();return result
            try:
                code,body=request('POST','/community/node-submissions',submission());self.assertEqual(code,202)
                self.assertEqual(json.loads(body)['status'],'pending');self.assertNotIn(b'operator@example.net',body)
                self.assertEqual(request('GET','/community/node-submissions')[0],404)
                self.assertEqual(request('GET','/submissions.sqlite3')[0],404)
                self.assertEqual(request('POST','/community/node-submissions',submission(),{'Origin':'https://other.example'})[0],403)
                self.assertEqual(request('POST','/community/node-submissions',submission(),{'Content-Type':'text/plain'})[0],415)
                self.assertEqual(request('POST','/community/node-submissions','X'*9000)[0],413)
                self.assertEqual(request('POST','/community/node-submissions','{bad json')[0],422)
                with queue.connect() as db:self.assertEqual(db.execute('SELECT COUNT(*) FROM submissions').fetchone()[0],1)
            finally:server.shutdown();server.server_close();thread.join()

    def test_approval_exports_only_reviewed_public_fields(self):
        with tempfile.TemporaryDirectory() as root:
            queue=service.Queue(Path(root)/'private');identity=queue.submit(submission(),'192.0.2.1')
            directory=Path(root)/'nodes.json';directory.write_text('{"schema_version":1,"nodes":[]}')
            command=[sys.executable,str(Path(service.__file__)), '--state',str(queue.root),'review',identity,'--decision','approve','--directory',str(directory)]
            self.assertNotEqual(subprocess.run(command,capture_output=True).returncode,0)
            result=subprocess.run(command+['--verification-note','Synthetic test; endpoints not contacted'],capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            public=json.loads(directory.read_text());self.assertEqual(public['nodes'][0]['whois'],'irr.example.net:43')
            self.assertNotIn('contact',public['nodes'][0]);self.assertNotIn('notes',public['nodes'][0]);self.assertNotIn('operator@example.net',directory.read_text())
            with queue.connect() as db:self.assertEqual(db.execute('SELECT status FROM submissions').fetchone()[0],'approved')


if __name__ == '__main__':unittest.main()
