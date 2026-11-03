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
            if args.screenshots:
                image=browser.call('Page.captureScreenshot',{'format':'png'},True)
                (args.screenshots/f'network-{width}.png').write_bytes(base64.b64decode(image['data']))
        browser.js('document.getElementById("site-query-kind").value="origin6";document.getElementById("site-query-value").value="AS3333";document.getElementById("site-query-form").requestSubmit()')
        browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
        assert browser.js('document.getElementById("query-kind").value')=='origin'
        assert browser.js('document.getElementById("query-family").value')=='6'
        assert browser.js('document.getElementById("query-value").value')=='AS3333'
        browser.js('document.getElementById("query-source").value="RIPE";document.getElementById("query-form").requestSubmit()')
        browser.wait('!document.getElementById("query-submit").disabled')
        assert '2001:' in browser.js('document.getElementById("query-result").textContent')
        browser.js('location.hash="overview"')
        browser.wait('Number(document.getElementById("source-chart")?.dataset.points)===19')
        assert not [event for event in browser.events if event.get('method')=='Runtime.exceptionThrown']
        print('Separate project/network identities, recorded benchmark graphs, live source graphs, mobile layouts, cross-site navigation and IPv6 search flow passed')
    finally:browser.close()
