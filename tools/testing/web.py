#!/usr/bin/env python3
"""Test the public bundled console against an isolated synthetic Rust registry."""
import argparse
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from urllib.parse import urlsplit
from harness import Registry,ROOT
from results import Result,write
sys.path.insert(0,str(ROOT/'tools'))
from check_dashboard import Browser

class Console(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.addClassCleanup(cls.close);cls.registry=Registry(ARGS.binary);cls.registry.start()
        cls.profile=tempfile.TemporaryDirectory(prefix='rirrd-console-check-')
        cls.browser=Browser(shutil.which('chromium')or shutil.which('google-chrome'),cls.profile.name,extra_args=['--proxy-server=http://127.0.0.1:9','--proxy-bypass-list=127.0.0.1;localhost'])
        cls.origin='http://'+cls.registry.http[0]+':'+str(cls.registry.http[1])
    @classmethod
    def close(cls):
        if hasattr(cls,'browser'):cls.browser.close()
        if hasattr(cls,'profile'):cls.profile.cleanup()
        if hasattr(cls,'registry'):cls.registry.close()
    def setUp(self):
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
        self.browser.wait('document.getElementById("live-objects").textContent==="14"')
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
    def test_schema_introspection_uses_actual_api(self):
        self.browser.js('location.hash="api";document.getElementById("load-schema").click()')
        self.browser.wait('document.getElementById("schema-result").textContent.includes("rpslObjects")')
    def test_form_controls_have_labels(self):
        missing=self.browser.js('Array.from(document.querySelectorAll("input,select,textarea")).filter(e=>e.type!=="hidden"&&!e.labels?.length&&!e.getAttribute("aria-label")&&!e.getAttribute("aria-labelledby")).map(e=>e.id)')
        self.assertEqual(missing,[])
    def test_all_views_fit_mobile_and_desktop(self):
        for width in [320,390,1440]:
            self.browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            for page in ['overview','query','sources','api','connect','system']:
                self.browser.js('location.hash='+json.dumps(page));self.browser.wait('!document.getElementById("page-'+page+'").hidden')
                self.assertTrue(self.browser.js('document.documentElement.scrollWidth<=innerWidth'),(width,page))
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
