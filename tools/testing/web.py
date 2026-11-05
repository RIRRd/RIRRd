#!/usr/bin/env python3
"""Test the public bundled console against an isolated synthetic Rust registry."""
import argparse
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time
import unittest
from urllib.parse import urlsplit
from harness import Registry,ROOT
from results import Result,write
sys.path.insert(0,str(ROOT/'tools'))
from check_dashboard import Browser

class Console(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.addClassCleanup(cls.close);cls.registry=Registry(ARGS.binary)
        cls.registry.config.write_text(cls.registry.config.read_text().replace('max_snapshot_bytes = 262144','max_snapshot_bytes = 1048576'))
        # Same set name, two valid definitions: default EXAMPLE is deliberately
        # empty; SECOND has a member. A successful empty answer is not absence.
        first=cls.registry.root/'first-priority.rpsl'
        first.write_text((ROOT/'tools/router-integration/fixture.rpsl').read_text()+'\nas-set: AS-PRIORITY\nsource: EXAMPLE\n')
        cls.registry.cli('import','snapshot','--source','EXAMPLE','--file',str(first))
        second=cls.registry.root/'second.rpsl'
        large='\nas-set: AS-LARGE\nmembers: AS4200000001\nsource: SECOND\n\n'+''.join(
            f'route6: fd00:{index:x}::/48\norigin: AS4200000001\nsource: SECOND\n\n' for index in range(6000))
        second.write_text(second.read_text()+'\nas-set: AS-PRIORITY\nmembers: AS64496\nsource: SECOND\n'+large)
        cls.registry.cli('import','snapshot','--source','SECOND','--file',str(second))
        cls.registry.start()
        cls.profile=tempfile.TemporaryDirectory(prefix='rirrd-console-check-')
        cls.browser=Browser(shutil.which('chromium')or shutil.which('google-chrome'),cls.profile.name,extra_args=['--proxy-server=http://127.0.0.1:9','--proxy-bypass-list=127.0.0.1;localhost'])
        cls.downloads=Path(cls.profile.name)/'downloads';cls.downloads.mkdir()
        cls.browser.call('Browser.setDownloadBehavior',{'behavior':'allow','downloadPath':str(cls.downloads),'eventsEnabled':True})
        cls.origin='http://'+cls.registry.http[0]+':'+str(cls.registry.http[1])
    @classmethod
    def close(cls):
        if hasattr(cls,'browser'):cls.browser.close()
        if hasattr(cls,'profile'):cls.profile.cleanup()
        if hasattr(cls,'registry'):cls.registry.close()
    def setUp(self):
        for path in self.downloads.iterdir():path.unlink()
        self.browser.events.clear();self.browser.call('Emulation.setDeviceMetricsOverride',{'width':1440,'height':1100,'deviceScaleFactor':1,'mobile':False},True)
        self.browser.call('Page.navigate',{'url':self.origin+'/ui/'},True)
        self.browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
    def tearDown(self):
        self.assertFalse([e for e in self.browser.events if e.get('method')=='Runtime.exceptionThrown'])
        for event in self.browser.events:
            if event.get('method')=='Network.requestWillBeSent':
                url=urlsplit(event['params']['request']['url'])
                if url.scheme in ['http','https']:self.assertIn(url.hostname,['127.0.0.1','localhost'])
    def test_real_counts_and_private_instance_identity(self):
        self.browser.wait('document.getElementById("live-objects").textContent==="6,017"')
        self.assertEqual(self.browser.js('document.getElementById("source-count").textContent'),'2')
        self.assertEqual(self.browser.js('document.getElementById("mode-badge").textContent'),'Private instance')
        self.assertNotIn('HYEHOST',self.browser.js('document.body.innerText'))
    def query(self,kind,value,family='4'):
        self.browser.js('location.hash="query";document.getElementById("query-kind").value='+json.dumps(kind)+';document.getElementById("query-family").value='+json.dumps(family)+';document.getElementById("query-value").value='+json.dumps(value)+';document.getElementById("query-form").requestSubmit()')
        self.browser.wait('!document.getElementById("query-submit").disabled')
        return self.browser.js('document.getElementById("query-result").textContent')
    def test_ipv4_origin_query(self):self.assertIn('192.0.2.0/24',self.query('origin','AS64496'))
    def test_ipv6_origin_query(self):self.assertIn('2001:db8:100::/48',self.query('origin','AS64496','6'))
    def test_recursive_members(self):self.assertIn('AS64497',self.query('recursive','AS64496:AS-DOCS'))
    def test_set_prefix_expansion(self):self.assertIn('198.51.100.0/24',self.query('set','AS64496:AS-DOCS'))
    def test_source_selection_is_explicit(self):
        self.browser.js('document.getElementById("query-source").value="SECOND"')
        self.assertIn('SECONDARY',self.query('object','AS64496'))
    def test_source_order_explains_empty_definitions_and_origin_union_without_extra_queries(self):
        described=self.browser.js('document.getElementById("query-source").getAttribute("aria-describedby")')
        self.assertIn('query-source-order',described);self.assertIn('query-source-precedence',described)
        self.assertIn('EXAMPLE → SECOND',self.browser.js('document.getElementById("query-source-order").textContent'))
        def requests():
            return sum(event.get('method')=='Network.requestWillBeSent' and urlsplit(event['params']['request']['url']).path=='/v1/whois/' for event in self.browser.events)
        before=requests()
        self.assertEqual(set(self.query('origin','AS64496').splitlines()),{'192.0.2.0/24','203.0.113.0/24'})
        self.assertEqual(requests(),before+1)
        result=self.query('recursive','AS-PRIORITY')
        self.assertIn('An empty list does not prove that the set is missing',result)
        self.assertIn('Select another IRR source',result)
        self.assertEqual(self.browser.js('document.getElementById("result-count").textContent'),'0 items')
        self.assertTrue(self.browser.js('document.getElementById("copy-result").disabled'))
        self.assertEqual(requests(),before+2)
        self.browser.js('document.getElementById("query-source").value="SECOND";document.getElementById("query-source").dispatchEvent(new Event("change"))')
        self.assertIn('Using SECOND only',self.browser.js('document.getElementById("query-source-order").textContent'))
        self.assertEqual(self.query('recursive','AS-PRIORITY'),'AS64496')
        self.assertEqual(requests(),before+3)
        self.assertIn('sources=SECOND',self.browser.js('document.getElementById("query-command").textContent'))
    def test_origin_empty_result_and_native_metrics_scope_are_clear(self):
        self.assertEqual(self.query('origin','AS64511'),'No prefixes were returned for this ASN in the selected sources.')
        self.assertIn('Requests rejected by the reverse proxy are excluded',self.browser.js('document.getElementById("http-admission-note").textContent'))
    def capture_exports(self):
        self.browser.js('''window.__copied=null;window.__downloadClicks=0;window.__revoked=[];
            Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async value=>{window.__copied=value;}}});
            const originalClick=HTMLAnchorElement.prototype.click;
            HTMLAnchorElement.prototype.click=function(){if(this.download){window.__downloadClicks++;window.__downloadUrl=this.href;}originalClick.call(this);};
            const revoke=URL.revokeObjectURL;URL.revokeObjectURL=function(value){window.__revoked.push(value);revoke.call(URL,value);};''')
    def downloaded(self):
        deadline=time.monotonic()+10;path=self.downloads/'rirrd-query-result.txt'
        while time.monotonic()<deadline:
            if path.exists() and not list(self.downloads.glob('*.crdownload')):return path.read_bytes()
            time.sleep(.025)
        self.fail('The complete Blob download did not finish')
    def test_large_real_result_has_bounded_preview_complete_exports_and_no_stale_result_after_failure(self):
        self.capture_exports()
        status,body,_=self.registry.request('/v1/whois/?q=%21a6AS-LARGE')
        self.assertEqual(status,200);self.assertGreater(len(body),65536)
        result=self.query('set','AS-LARGE','6')
        self.assertEqual(len(result.splitlines()),1000);self.assertLessEqual(len(result.encode()),65536)
        self.assertIn('1,000 of 6,000',self.browser.js('document.getElementById("result-preview-note").textContent'))
        self.assertFalse(self.browser.js('document.getElementById("result-preview-note").hidden'))
        self.assertEqual(self.browser.js('document.getElementById("result-count").textContent'),'6,000 items')
        self.browser.js('document.getElementById("copy-result").click()');self.browser.wait('window.__copied!==null')
        self.assertEqual(self.browser.js('window.__copied'),body.decode())
        self.browser.js('document.getElementById("download-result").click()')
        self.assertEqual(self.downloaded(),body)
        url=self.browser.js('window.__downloadUrl')
        self.browser.js('document.getElementById("query-source").add(new Option("UNKNOWN","UNKNOWN"));document.getElementById("query-source").value="UNKNOWN"')
        self.assertEqual(self.query('set','AS-LARGE','6'),'No usable response was returned.')
        self.assertIn('HTTP 400',self.browser.js('document.getElementById("query-status").textContent'))
        self.assertTrue(self.browser.js('document.getElementById("copy-result").disabled&&document.getElementById("download-result").disabled&&document.getElementById("result-preview-note").hidden'))
        self.assertIn(url,self.browser.js('window.__revoked'))
        self.browser.js('window.__copied=null;document.getElementById("copy-result").dispatchEvent(new Event("click"));document.getElementById("download-result").dispatchEvent(new Event("click"))')
        self.assertIsNone(self.browser.js('window.__copied'));self.assertEqual(self.browser.js('window.__downloadClicks'),1)
    def install_incomplete_response(self, timeout=False):
        self.browser.js('''window.__queryAttempts=0;window.__queryTimeout=null;
            const originalFetch=window.fetch;window.fetch=(url,options)=>{
              if(!String(url).startsWith('/v1/whois/'))return originalFetch(url,options);
              window.__queryAttempts++;
              return Promise.resolve(new Response(new ReadableStream({start(sink){
                sink.enqueue(new TextEncoder().encode('192.0.2.0/24 '));
                options.signal.addEventListener('abort',()=>sink.error(new DOMException('Cancelled','AbortError')),{once:true});
              }})));
            };
            const originalTimeout=window.setTimeout;window.setTimeout=(callback,delay,...args)=>{
              if(delay>=60000){window.__queryTimeout=delay;''' + ('delay=50;' if timeout else '') + '''}
              return originalTimeout(callback,delay,...args);
            };''')
    def test_cancel_incomplete_query_clears_previous_payload_and_never_retries(self):
        self.capture_exports();self.query('origin','AS64496');self.browser.js('document.getElementById("download-result").click()')
        self.downloaded();url=self.browser.js('window.__downloadUrl')
        self.install_incomplete_response()
        self.browser.js('document.getElementById("query-kind").value="set";document.getElementById("query-value").value="AS-LARGE";document.getElementById("query-form").requestSubmit()')
        self.browser.wait('window.__queryAttempts===1')
        self.assertFalse(self.browser.js('document.getElementById("query-cancel").hidden'))
        self.assertTrue(self.browser.js('document.getElementById("copy-result").disabled&&document.getElementById("download-result").disabled'))
        self.assertIn(url,self.browser.js('window.__revoked'))
        self.browser.js('document.getElementById("query-cancel").click()');self.browser.wait('!document.getElementById("query-submit").disabled')
        self.assertIn('Query cancelled. No partial result',self.browser.js('document.getElementById("query-status").textContent'))
        self.assertEqual(self.browser.js('document.getElementById("query-result").textContent'),'No usable response was returned.')
        self.assertEqual(self.browser.js('window.__queryTimeout'),120000);self.assertEqual(self.browser.js('window.__queryAttempts'),1)
        self.assertTrue(self.browser.js('document.getElementById("query-cancel").hidden'))
    def test_finite_set_query_timeout_discards_partial_body_without_retry(self):
        self.install_incomplete_response(timeout=True)
        self.assertEqual(self.query('set','AS-LARGE'),'No usable response was returned.')
        self.assertIn('query timed out',self.browser.js('document.getElementById("query-status").textContent'))
        self.assertEqual(self.browser.js('window.__queryTimeout'),120000);self.assertEqual(self.browser.js('window.__queryAttempts'),1)
        self.assertTrue(self.browser.js('document.getElementById("copy-result").disabled&&document.getElementById("download-result").disabled'))
    def test_schema_introspection_uses_actual_api(self):
        self.browser.js('location.hash="api";document.getElementById("load-schema").click()')
        self.browser.wait('document.getElementById("schema-result").textContent.includes("rpslObjects")')
    def test_form_controls_have_labels(self):
        missing=self.browser.js('Array.from(document.querySelectorAll("input,select,textarea")).filter(e=>e.type!=="hidden"&&!e.labels?.length&&!e.getAttribute("aria-label")&&!e.getAttribute("aria-labelledby")).map(e=>e.id)')
        self.assertEqual(missing,[])
    def test_all_views_fit_mobile_and_desktop(self):
        # The small registry fixture hid a real layout defect: a 14-million
        # object total split its last digit at intermediate desktop widths.
        # Feed realistic totals through the ordinary telemetry/render path.
        self.browser.wait('document.getElementById("live-objects").textContent==="6,017" && !document.getElementById("refresh").disabled')
        self.browser.js('''const metricFetch=window.fetch;window.fetch=async(...args)=>{
          const response=await metricFetch(...args);
          if(new URL(args[0],location.href).pathname!=="/ui/mirrors.json")return response;
          const value=await response.json();value.total_objects=14000000;
          value.class_counts={EXAMPLE:{route:3342010,route6:1342007,"as-set":77433,person:9238550}};
          return new Response(JSON.stringify(value),{status:response.status,headers:response.headers});
        };document.getElementById("refresh").click();''')
        self.browser.wait('document.getElementById("live-objects").textContent==="14,000,000"')
        for width in [320,390,720,721,900,1000,1200,1280,1360,1361,1440]:
            self.browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            for page in ['overview','query','sources','api','connect','system']:
                self.browser.js('location.hash='+json.dumps(page));self.browser.wait('!document.getElementById("page-'+page+'").hidden')
                self.assertTrue(self.browser.js('document.documentElement.scrollWidth<=innerWidth'),(width,page))
                if page=='overview':
                    metrics=self.browser.js('''Array.from(document.querySelectorAll("#overview-metrics strong")).map(node=>{
                      const range=document.createRange();range.selectNodeContents(node);
                      const value=range.getBoundingClientRect(),box=node.getBoundingClientRect();
                      return {text:node.textContent,lines:range.getClientRects().length,
                        fits:value.left>=box.left-.5&&value.right<=box.right+.5};
                    })''')
                    self.assertEqual(metrics[0]['text'],'14,000,000')
                    for metric in metrics:
                        self.assertEqual(metric['lines'],1,(width,metric))
                        self.assertTrue(metric['fits'],(width,metric))
    def test_retired_and_unknown_sections_show_registry(self):
        for page in ['rpki','nonexistent']:
            self.browser.js('location.hash='+json.dumps(page));self.browser.wait('!document.getElementById("page-overview").hidden')
        self.assertFalse(self.browser.js('Boolean(document.querySelector("[data-page=rpki]"))'))
    def test_local_router_examples_and_http_commands(self):
        self.browser.js('location.hash="api"');self.browser.wait('!document.getElementById("page-api").hidden')
        text=self.browser.js('document.getElementById("command-examples").textContent')
        self.assertIn('!6AS64496',text);self.assertIn(self.origin,text)
        self.browser.js('location.hash="connect"');self.browser.wait('!document.getElementById("page-connect").hidden')
        self.assertEqual(self.browser.js('document.getElementById("router-commands").children.length'),4)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,required=True);parser.add_argument('--work',type=Path,required=True);parser.add_argument('--report',type=Path,required=True);ARGS=parser.parse_args()
    if ARGS.work.resolve().is_relative_to(ROOT):parser.error('Keep browser fixtures outside Git')
    result=unittest.TextTestRunner(verbosity=2,resultclass=Result).run(unittest.defaultTestLoader.loadTestsFromTestCase(Console));write(ARGS.report,result.records);raise SystemExit(not result.wasSuccessful())
