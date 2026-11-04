#!/usr/bin/env python3
"""Exercise both built websites with a synthetic registry and private queue only."""
import argparse
import datetime
import http.client
import hashlib
import http.server
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from urllib.parse import urlsplit

from harness import Registry, ROOT
from results import Result, write
sys.path.insert(0,str(ROOT/'tools'))
sys.path.insert(0,str(ROOT/'tools/live-service'))
from check_dashboard import Browser
import gateway


class Websites(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cleanup=[]
        cls.addClassCleanup(cls.close)
        cls.registry=Registry(ARGS.binary);cls.cleanup.append(cls.registry.close);cls.registry.start()
        cls.work=ARGS.work.resolve();cls.work.mkdir(parents=True,exist_ok=True)
        cls.queue_dir=tempfile.TemporaryDirectory(prefix='private-queue-',dir=cls.work);cls.cleanup.append(cls.queue_dir.cleanup)
        for name in ['project','community']:
            subprocess.run([sys.executable,str(ROOT/f'site/{name}/build.py'),'--allow-dirty','--output',str(cls.work/name)],cwd=ROOT,check=True,timeout=30)
        status,raw,_=cls.registry.request('/ui/mirrors.json');assert status==200
        snapshot=json.loads(raw)
        cls.state=gateway.State(cls.work,cls.registry.http[1])
        # Fixture counts come from the real daemon. This does not read the live
        # VM's systemd cgroup, production generation, cache or telemetry files.
        cls.state.telemetry={**snapshot,'service':'online','sources':[{'source':name,'state':'loaded','objects':sum(counts.values()),'http_access':True} for name,counts in snapshot['class_counts'].items()], 'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        cls.state.sample(cls.state.telemetry)
        cls.proxy=gateway.Server(('127.0.0.1',0),cls.state);cls.serve(cls.proxy)
        cls.requests=[]
        class Handler(http.server.SimpleHTTPRequestHandler):
            def __init__(self,*args,**kw):super().__init__(*args,directory=args[2].root,**kw)
            def log_message(self,*_):pass
            def do_POST(self):self.dispatch()
            def do_GET(self):self.dispatch()
            def dispatch(self):
                path=urlsplit(self.path).path;cls.requests.append((self.command,path))
                if path.startswith(('/v1/','/graphql','/dashboard/meta.json','/dashboard/live.json','/dashboard/history.json','/community/node-submissions')):
                    port=cls.queue.server_port if path=='/community/node-submissions' else cls.proxy.server_port
                    connection=http.client.HTTPConnection('127.0.0.1',port,timeout=10)
                    try:
                        data=self.rfile.read(int(self.headers.get('Content-Length',0)))
                        headers={'Content-Type':self.headers.get('Content-Type','application/json')}
                        if self.headers.get('Origin'):headers['Origin']=self.headers['Origin']
                        connection.request(self.command,self.path,body=data or None,headers=headers)
                        response=connection.getresponse();body=response.read();self.send_response(response.status);self.send_header('Content-Type',response.getheader('Content-Type'));self.send_header('Content-Length',str(len(body)));self.end_headers()
                        try:self.wfile.write(body)
                        except (BrokenPipeError,ConnectionResetError):pass
                    finally:connection.close()
                elif self.command=='GET':super().do_GET()
                else:self.send_error(404)
        cls.sites={}
        for name in ['project','community']:
            server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);server.daemon_threads=True;server.root=str(cls.work/name);cls.serve(server);cls.sites[name]='http://127.0.0.1:'+str(server.server_port)
        spec=importlib.util.spec_from_file_location('community_submission',ROOT/'site/community/service.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        cls.queue=module.Server(('127.0.0.1',0),module.Queue(cls.queue_dir.name),cls.sites['community']);cls.serve(cls.queue)
        cls.profile=tempfile.TemporaryDirectory(prefix='rirrd-ci-browser-');cls.cleanup.append(cls.profile.cleanup)
        browser=shutil.which('chromium')or shutil.which('google-chrome');assert browser,'Chromium or Chrome is required'
        # External requests have no working proxy. Loopback fixture requests
        # bypass it; no test needs DNS, production endpoints or public RPKI feeds.
        cls.browser=Browser(browser,cls.profile.name,extra_args=['--proxy-server=http://127.0.0.1:9','--proxy-bypass-list=127.0.0.1;localhost']);cls.cleanup.append(cls.browser.close)

    @classmethod
    def serve(cls,server):
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        cls.cleanup.append(lambda:(server.shutdown(),server.server_close(),thread.join(timeout=3)))

    @classmethod
    def close(cls):
        for cleanup in reversed(cls.cleanup):cleanup()

    def setUp(self):
        self.browser.events.clear()
        self.browser.call('Emulation.setDeviceMetricsOverride',{'width':1440,'height':1100,'deviceScaleFactor':1,'mobile':False},True)
        with self.state.lock:self.state.clients.clear()

    def tearDown(self):
        errors=[e for e in self.browser.events if e.get('method')=='Runtime.exceptionThrown']
        self.assertFalse(errors,errors)
        for e in self.browser.events:
            if e.get('method')=='Network.requestWillBeSent':
                url=e['params']['request']['url'];parsed=urlsplit(url)
                if parsed.scheme in ['http','https']:self.assertIn(parsed.hostname,['127.0.0.1','localhost'],url)

    def open(self,product,path='/',condition='document.readyState==="complete"'):
        self.browser.call('Page.navigate',{'url':'about:blank'},True)
        self.browser.call('Page.navigate',{'url':self.sites[product]+path},True)
        self.browser.wait(condition)

    def console(self,path='/dashboard/#overview'):
        self.open('community',path,'document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')

    def test_build_manifests_match_every_served_asset(self):
        for product in ['project','community']:
            manifest=json.loads((self.work/product/'build.json').read_text())
            for path,digest in manifest['files'].items():
                url=urlsplit(self.sites[product]);connection=http.client.HTTPConnection(url.hostname,url.port,timeout=5)
                try:
                    connection.request('GET','/'+path);response=connection.getresponse();body=response.read()
                    self.assertEqual(response.status,200,(product,path))
                    self.assertEqual(hashlib.sha256(body).hexdigest(),digest,(product,path))
                finally:connection.close()

    def test_form_fields_have_accessible_names(self):
        for product,path in [('community','/'),('community','/dashboard/#query')]:
            self.open(product,path,'document.readyState==="complete"')
            missing=self.browser.js('Array.from(document.querySelectorAll("input,select,textarea")).filter(e=>e.type!=="hidden" && !e.labels?.length && !e.getAttribute("aria-label") && !e.getAttribute("aria-labelledby")).map(e=>e.id)')
            self.assertEqual(missing,[])

    def test_project_benchmark_graphs_and_separate_brand(self):
        self.open('project',condition='document.querySelectorAll("#benchmark-queries tr").length===2')
        self.assertEqual(self.browser.js('document.querySelectorAll(".chart svg").length'),3)
        self.assertTrue(self.browser.js('document.title.includes("RIRRd") && !document.getElementById("node-form")'))
        self.browser.wait('document.getElementById("showcase-objects").textContent==="14"')

    def test_project_testing_page_is_accessible_on_mobile(self):
        self.open('project','/testing/','document.getElementById("verification-status") && !document.getElementById("verification-status").textContent.includes("Loading")')
        for width in [320,390,1440]:
            self.browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            self.assertTrue(self.browser.js('document.documentElement.scrollWidth<=innerWidth'),width)
        self.assertTrue(self.browser.js('Array.from(document.links).some(a=>a.getAttribute("href")==="report.json")'))

    def test_snapshot_does_not_claim_unverified_github_success(self):
        self.open('project','/testing/','document.getElementById("verification-status") && !document.getElementById("verification-status").textContent.includes("Loading")')
        snapshot=json.loads((self.work/'project/testing/report.json').read_text())
        expected='exported from the linked GitHub Actions' if snapshot.get('ci_status')=='recorded_run' else 'has not been verified'
        self.assertIn(expected,self.browser.js('document.getElementById("ci-verification-note").textContent'))

    def test_community_counters_come_from_the_isolated_database(self):
        self.open('community',condition='document.getElementById("network-objects")?.textContent==="14"')
        self.assertEqual(self.browser.js('document.getElementById("network-sources").textContent'),'2')
        self.assertEqual(self.browser.js('document.getElementById("network-sets").textContent'),'2')
        self.browser.wait('document.getElementById("node-list").querySelectorAll("article").length===1')

    def test_homepage_query_executes_once_and_defers_monitoring(self):
        self.open('community',condition='document.getElementById("site-query-form")!==null')
        self.browser.js('document.getElementById("site-query-kind").value="origin6";document.getElementById("site-query-value").value="AS64496";document.getElementById("site-query-form").requestSubmit()')
        self.browser.wait('document.getElementById("query-status")?.textContent.includes("Ready to copy")')
        self.assertIn('2001:db8:100::/48',self.browser.js('document.getElementById("query-result").textContent'))
        paths=self.browser.js('performance.getEntriesByType("resource").map(r=>new URL(r.name).pathname)')
        self.assertEqual(paths.count('/v1/whois/'),1)
        self.assertFalse(set(paths)&{'/dashboard/live.json','/dashboard/history.json','/v1/rpki/status'})
        self.assertNotIn('run=1',self.browser.js('location.search'))

    def test_prepared_link_does_not_query_automatically(self):
        self.console('/dashboard/?task=origin6&value=AS64496#query')
        self.assertEqual(self.browser.js('document.getElementById("query-result").textContent'),'No query submitted.')
        self.assertFalse(self.browser.js('performance.getEntriesByType("resource").some(r=>new URL(r.name).pathname==="/v1/whois/")'))

    def test_unknown_console_section_loads_overview(self):
        self.console('/dashboard/#unknown-section')
        self.browser.wait('document.getElementById("live-objects").textContent==="14"')
        self.assertTrue(self.browser.js('!document.getElementById("page-overview").hidden'))

    def test_disabled_rpki_cannot_be_submitted(self):
        self.console('/dashboard/#rpki')
        self.browser.wait('document.getElementById("rpki-detail").textContent.includes("not enabled")')
        self.assertTrue(self.browser.js('document.getElementById("rpki-submit").disabled && document.getElementById("rpki-batch-submit").disabled'))
        self.assertNotIn('NOT_FOUND',self.browser.js('document.getElementById("rpki-verdict").textContent'))

    def test_graphql_workbench_introspection(self):
        self.console('/dashboard/#api')
        self.browser.js('document.getElementById("load-schema").click()')
        self.browser.wait('!document.getElementById("load-schema").disabled')
        self.assertIn('rpslObjects',self.browser.js('document.getElementById("schema-result").textContent'))

    def test_source_browse_and_mobile_views(self):
        self.console('/dashboard/#sources')
        self.browser.wait('document.getElementById("browse-source").options.length===2')
        self.browser.js('document.getElementById("browse-source").value="EXAMPLE";document.getElementById("browse-class").value="as-set";document.getElementById("browse-form").requestSubmit()')
        self.browser.wait('document.getElementById("browse-results").children.length===2')
        for width in [320,390,1440]:
            self.browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            for page in ['overview','query','sources','rpki','api','connect','system']:
                self.browser.js('location.hash='+json.dumps(page));self.browser.wait('!document.getElementById("page-'+page+'").hidden')
                self.assertTrue(self.browser.js('document.documentElement.scrollWidth<=innerWidth'),(width,page))

    def test_site_mobile_navigation_and_local_documentation(self):
        for name,path in [('project','/'),('project','/docs/beta/'),('community','/'),('community','/guides/api/')]:
            self.browser.call('Emulation.setDeviceMetricsOverride',{'width':390,'height':1100,'deviceScaleFactor':1,'mobile':True},True)
            self.open(name,path,'document.getElementById("site-navigation")?.hidden===true')
            self.browser.js('document.querySelector(".menu-toggle").click()')
            self.assertFalse(self.browser.js('document.getElementById("site-navigation").hidden'))
            self.browser.js('document.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape"}))')
            self.assertTrue(self.browser.js('document.getElementById("site-navigation").hidden'))
            self.assertTrue(self.browser.js('document.documentElement.scrollWidth<=innerWidth'))

    def test_submission_goes_only_to_the_private_fixture_queue(self):
        self.open('community',condition='document.getElementById("node-form")!==null')
        fields={'node-name':'CI synthetic node','operator':'Example operator','country':'United Kingdom','country-code':'GB','node-region':'Europe','provider':'Example host','sponsor':'Example sponsor','contact':'operator@example.net','whois-host':'irr.example.net','node-https':'https://irr.example.net/'}
        self.browser.js(';'.join('document.getElementById('+json.dumps(k)+').value='+json.dumps(v)for k,v in fields.items())+';document.getElementById("authorised").checked=true;document.getElementById("node-form").requestSubmit()')
        self.browser.wait('document.getElementById("submission-status").textContent.includes("Reference:")')
        self.assertNotIn('operator@example.net',self.browser.js('document.getElementById("node-list").textContent'))
        self.assertNotIn('CI synthetic node',self.browser.js('document.getElementById("node-list").textContent'))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,required=True);parser.add_argument('--work',type=Path,required=True);parser.add_argument('--report',type=Path,required=True)
    ARGS=parser.parse_args()
    if ARGS.work.resolve().is_relative_to(ROOT):parser.error('Keep private web state outside Git')
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(Websites)
    result=unittest.TextTestRunner(verbosity=2,resultclass=Result).run(suite);write(ARGS.report,result.records)
    raise SystemExit(not result.wasSuccessful())
