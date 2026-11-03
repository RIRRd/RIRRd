#!/usr/bin/env python3
"""Check the separate showcase/network products and real SVG charts in Chromium."""
import argparse
import base64
import json
from pathlib import Path
import shutil
import tempfile
from check_dashboard import Browser

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--project',required=True)
parser.add_argument('--community',required=True)
parser.add_argument('--screenshots',type=Path)
args=parser.parse_args()
with tempfile.TemporaryDirectory(prefix='rirrd-two-sites-browser-') as profile:
    browser=Browser(shutil.which('chromium') or shutil.which('google-chrome'),profile)
    try:
        browser.call('Page.navigate',{'url':args.project.rstrip('/')+'/'},True)
        browser.wait('document.querySelectorAll("#benchmark-queries tr").length===2')
        assert browser.js('document.title.includes("RIRRd")')
        assert browser.js('document.getElementById("node-form")===null')
        assert browser.js('document.querySelectorAll(".chart svg").length')==3
        assert browser.js('Array.from(document.links).some(a=>a.href==="https://irrd.tools/")')
        for width in [320,390,1440]:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            assert browser.js('document.documentElement.scrollWidth<=innerWidth'),('project',width)
            if args.screenshots:
                args.screenshots.mkdir(parents=True,exist_ok=True)
                image=browser.call('Page.captureScreenshot',{'format':'png'},True)
                (args.screenshots/f'project-{width}.png').write_bytes(base64.b64decode(image['data']))
        browser.call('Page.navigate',{'url':args.community.rstrip('/')+'/'},True)
        browser.wait('document.getElementById("network-nodes")?.textContent==="1"')
        browser.wait('Number(document.getElementById("network-source-chart")?.dataset.points)>0')
        assert browser.js('document.title.includes("irrd.tools")')
        assert browser.js('Array.from(document.links).some(a=>a.href==="https://rirrd.co.uk/")')
        assert browser.js('document.getElementById("node-form")!==null')
        for width in [320,390,1440]:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            assert browser.js('document.documentElement.scrollWidth<=innerWidth'),('network',width)
            assert browser.js('Array.from(document.querySelectorAll(".summary strong")).every(e=>{const r=document.createRange();r.selectNodeContents(e);return r.getClientRects().length===1 && e.scrollWidth<=e.clientWidth})'),('counter wrapping',width)
            if args.screenshots:
                image=browser.call('Page.captureScreenshot',{'format':'png'},True)
                (args.screenshots/f'network-{width}.png').write_bytes(base64.b64decode(image['data']))
        browser.js('document.getElementById("site-query-kind").value="origin6";document.getElementById("site-query-value").value="AS3333";document.getElementById("site-query-form").requestSubmit()')
        browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
        assert browser.js('document.getElementById("query-kind").value')=='origin'
        assert browser.js('document.getElementById("query-family").value')=='6'
        assert browser.js('document.getElementById("query-source").value')==''
        assert browser.js('document.getElementById("query-value").value')=='AS3333'
        browser.wait('document.getElementById("query-status").textContent.includes("Ready to copy")')
        assert 'run=1' not in browser.js('location.search')
        requests=browser.js('performance.getEntriesByType("resource").map(r=>new URL(r.name).pathname)')
        assert requests.count('/v1/whois/')==1,requests
        assert not any(p in requests for p in ['/dashboard/live.json','/dashboard/history.json','/v1/rpki/status']),requests
        assert '2001:' in browser.js('document.getElementById("query-result").textContent')
        preset=browser.js('location.href').replace('#query','&preset_check=1#query')
        browser.call('Page.navigate',{'url':preset},True)
        browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
        assert browser.js('document.getElementById("query-result").textContent')=='No query submitted.'
        assert not browser.js('performance.getEntriesByType("resource").some(r=>new URL(r.name).pathname==="/v1/whois/")')
        # Delay read-only responses in this private browser tab to exercise real
        # form races, including a transport that resolves after cancellation.
        browser.js('''window.originalFetch=window.fetch;window.flowCalls=0;window.fetch=(url,options)=>String(url).startsWith('/v1/whois/')?new Promise(resolve=>{const n=++flowCalls;setTimeout(()=>resolve(new Response(n===1?'192.0.2.0/24 198.51.100.0/24':'aut-num: AS3333\\nsource: RIPE')),n===1?120:10)}):originalFetch(url,options);document.getElementById('query-kind').value='origin';document.getElementById('query-form').requestSubmit();document.getElementById('query-kind').value='object';document.getElementById('query-form').requestSubmit();''')
        browser.wait('flowCalls===2 && !document.getElementById("query-submit").disabled')
        browser.js('new Promise(resolve=>setTimeout(resolve,160))')
        assert browser.js('document.getElementById("query-result").textContent')=='aut-num: AS3333\nsource: RIPE'
        browser.js('''flowCalls=0;document.getElementById('query-kind').value='origin';document.getElementById('query-form').requestSubmit();document.getElementById('query-kind').value='object';''')
        browser.wait('!document.getElementById("query-submit").disabled')
        assert browser.js('document.getElementById("query-result").textContent')=='192.0.2.0/24\n198.51.100.0/24'
        browser.js("window.fetch=originalFetch")
        browser.js('location.hash="not-a-page"')
        browser.wait('Number(document.getElementById("source-chart")?.dataset.points)===19')
        assert not [event for event in browser.events if event.get('method')=='Runtime.exceptionThrown']
        print('Separate project/network identities, recorded benchmark graphs, live source graphs, mobile layouts, cross-site navigation and IPv6 search flow passed')
    finally:browser.close()
