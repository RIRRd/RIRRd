#!/usr/bin/env python3
"""Check the community website and shared query console with a real browser."""
import argparse
import base64
import json
from pathlib import Path
import shutil
import tempfile
from check_dashboard import Browser

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url',required=True)
parser.add_argument('--screenshots',type=Path)
parser.add_argument('--submit-preview',action='store_true',help='Use only with an isolated temporary queue')
args=parser.parse_args();base=args.url.rstrip('/')
with tempfile.TemporaryDirectory(prefix='rirrd-community-browser-') as profile:
    browser=Browser(shutil.which('chromium') or shutil.which('google-chrome'),profile)
    try:
        browser.call('Page.navigate',{'url':base+'/'},True)
        browser.wait('document.getElementById("network-nodes")?.textContent === "1"')
        assert browser.js('document.getElementById("node-list").textContent.includes("HYEHOST LLC")')
        assert browser.js('document.body.textContent.includes("Self-host RIRRd") && document.body.textContent.includes("Submit a community node")')
        assert not browser.js('document.body.innerText.includes("127.0.0.1")')
        for width in [320,390,1440]:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            assert browser.js('document.documentElement.scrollWidth<=innerWidth'),('community',width)
            if args.screenshots:
                args.screenshots.mkdir(parents=True,exist_ok=True);browser.js('scrollTo(0,0)')
                shot=browser.call('Page.captureScreenshot',{'format':'png'},True);(args.screenshots/f'community-{width}.png').write_bytes(base64.b64decode(shot['data']))
        if args.submit_preview:
            fields={'node-name':'Preview test node','operator':'Example operator','country':'United Kingdom','country-code':'GB','node-region':'Europe','provider':'Example host','sponsor':'Example sponsor','contact':'operator@example.net','whois-host':'irr.example.net','node-https':'https://irr.example.net/'}
            browser.js(';'.join('document.getElementById('+json.dumps(key)+').value='+json.dumps(value) for key,value in fields.items())+';document.getElementById("authorised").checked=true;document.getElementById("node-form").requestSubmit()')
            browser.wait('document.getElementById("submission-status").textContent.includes("Reference:")')
            assert 'operator@example.net' not in browser.js('document.getElementById("node-list").textContent')
        browser.call('Page.navigate',{'url':base+'/dashboard/?task=origin6#query'},True)
        browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
        assert browser.js('document.getElementById("query-kind").value')=='origin'
        assert browser.js('document.getElementById("query-family").value')=='6'
        def query(value,kind='object',family='4'):
            browser.js('document.getElementById("query-value").value='+json.dumps(value)+';document.getElementById("query-kind").value='+json.dumps(kind)+';document.getElementById("query-family").value='+json.dumps(family)+';document.getElementById("query-source").value="RIPE";document.getElementById("query-form").requestSubmit()')
            browser.wait('!document.getElementById("query-submit").disabled')
            return browser.js('document.getElementById("query-result").textContent')
        assert 'AS3333' in query('AS3333')
        assert '193.0.0.0/21' in query('AS3333','origin')
        assert '2001:' in query('AS3333','origin','6')
        assert 'AS3333' in query('AS-RIPENCC','recursive')
        assert '193.0.0.0/21' in query('AS-RIPENCC','set')
        assert '2001:' in query('AS-RIPENCC','set','6')
        assert 'No matching' in query('RIRRD-ABSENT-UI-CHECK')
        browser.js('location.hash="sources"')
        browser.wait('document.querySelectorAll("#source-rows tr").length===19')
        browser.js('document.getElementById("source-search").value="ARIN";document.getElementById("source-search").dispatchEvent(new Event("input"))')
        assert browser.js('document.querySelectorAll("#source-rows tr").length')==1
        browser.js('document.getElementById("source-search").value="";document.getElementById("source-search").dispatchEvent(new Event("input"));document.getElementById("browse-source").value="RIPE";document.getElementById("browse-form").requestSubmit()')
        browser.wait('document.getElementById("browse-results").children.length>0')
        browser.js('location.hash="api"');browser.wait('!document.getElementById("page-api").hidden')
        assert browser.js('document.getElementById("command-examples").children.length')==6
        assert '!6AS3333' in browser.js('document.getElementById("command-examples").textContent')
        browser.js('location.hash="rpki"');browser.wait('!document.getElementById("rpki-submit").disabled')
        browser.js('document.getElementById("rpki-form").requestSubmit()');browser.wait('document.getElementById("rpki-verdict").textContent==="VALID"')
        assert not browser.js('document.body.innerText.includes("127.0.0.1")')
        for width in [320,390,1440]:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
            for page in ['overview','query','sources','rpki','api','connect','system']:
                browser.js('location.hash='+json.dumps(page));browser.wait('!document.getElementById("page-'+page+'").hidden')
                assert browser.js('document.documentElement.scrollWidth<=innerWidth'),(width,page)
            if args.screenshots:
                browser.js('location.hash="overview"');browser.wait('!document.getElementById("page-overview").hidden')
                shot=browser.call('Page.captureScreenshot',{'format':'png'},True);(args.screenshots/f'console-{width}.png').write_bytes(base64.b64decode(shot['data']))
        assert not [event for event in browser.events if event.get('method')=='Runtime.exceptionThrown']
        print('Community directory, private submission preview, seven console views, IPv4/IPv6/set/ASN queries, source filtering/browsing, commands and RPKI passed at 320/390/1440px')
    finally:browser.close()
