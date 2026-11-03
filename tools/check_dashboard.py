#!/usr/bin/env python3
"""Exercise the real read-only dashboard in Chromium over DevTools pipes."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import signal
import shlex
import shutil
import subprocess
import tempfile
import time
from urllib.parse import urlsplit


class Browser:
    def __init__(self, executable, profile):
        command=[executable,'--headless','--no-sandbox','--disable-gpu','--disable-background-networking','--no-first-run','--remote-debugging-pipe','--user-data-dir='+profile]
        self.child=subprocess.Popen(['bash','-c','exec '+shlex.join(command)+' 3<&0 4>&1'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,start_new_session=True)
        self.sequence=0;self.buffer=b'';self.events=[]
        target=self.call('Target.createTarget',{'url':'about:blank'})['targetId']
        self.session=self.call('Target.attachToTarget',{'targetId':target,'flatten':True})['sessionId']
        self.call('Page.enable',session=True);self.call('Network.enable',session=True)
        self.call('Runtime.enable',session=True)

    def call(self,method,params=None,session=False):
        self.sequence+=1
        message={'id':self.sequence,'method':method,'params':params or {}}
        if session:message['sessionId']=self.session
        self.child.stdin.write(json.dumps(message).encode()+b'\0');self.child.stdin.flush()
        deadline=time.monotonic()+25
        while True:
            if b'\0' not in self.buffer:
                remaining=deadline-time.monotonic()
                if remaining<=0 or not select.select([self.child.stdout],[],[],remaining)[0]:raise RuntimeError('browser timed out: '+method)
                block=os.read(self.child.stdout.fileno(),65536)
                if not block:raise RuntimeError('browser exited: '+method)
                self.buffer+=block;continue
            raw,self.buffer=self.buffer.split(b'\0',1)
            response=json.loads(raw)
            if response.get('id')==self.sequence:
                if 'error' in response:raise RuntimeError(response['error'])
                return response.get('result',{})
            self.events.append(response)

    def js(self,expression):
        result=self.call('Runtime.evaluate',{'expression':expression,'returnByValue':True,'awaitPromise':True},session=True)
        if 'exceptionDetails' in result:raise RuntimeError(result['exceptionDetails'])
        return result['result'].get('value')

    def wait(self,condition):
        self.js('new Promise((resolve,reject)=>{const end=Date.now()+10000;function check(){if('+condition+'){resolve(true)}else if(Date.now()>end){reject(new Error("UI condition timed out"))}else{setTimeout(check,25)}}check()})')

    def close(self):
        try:os.killpg(self.child.pid,signal.SIGTERM)
        except ProcessLookupError:pass
        try:self.child.wait(timeout=5)
        except subprocess.TimeoutExpired:self.child.kill();self.child.wait()
        # Chromium subprocesses may finish profile writes after the browser exits.
        try:os.killpg(self.child.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        self.child.stdin.close();self.child.stdout.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url',required=True,help='fixture-loaded live /ui/ or synthetic preview /dashboard/')
    parser.add_argument('--mode',choices=['live','preview','public'],required=True)
    parser.add_argument('--browser',default=shutil.which('chromium') or shutil.which('google-chrome'))
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    if not args.browser:parser.error('Chromium or Google Chrome is required')
    origin=urlsplit(args.url);origin=origin.scheme+'://'+origin.netloc
    with tempfile.TemporaryDirectory(prefix='rirrd-browser-') as profile:
        browser=Browser(args.browser,profile)
        try:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':1440,'height':1100,'deviceScaleFactor':1,'mobile':False},True)
            browser.call('Page.navigate',{'url':args.url},True)
            browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
            assert browser.js('document.getElementById("mode-badge").textContent')==('Synthetic preview' if args.mode=='preview' else 'Live · Experimental' if args.mode=='public' else 'Private instance')
            def query(value,kind='object',family='4',source=None):
                source=source or ('RIPE' if args.mode=='public' else 'EXAMPLE')
                browser.js('location.hash="query";document.getElementById("query-value").value='+json.dumps(value)+';document.getElementById("query-kind").value='+json.dumps(kind)+';document.getElementById("query-family").value='+json.dumps(family)+';document.getElementById("query-source").value='+json.dumps(source)+';document.getElementById("query-form").requestSubmit()')
                browser.wait('!document.getElementById("query-submit").disabled && document.getElementById("query-status").textContent!=="Querying…"')
                return browser.js('document.getElementById("query-result").textContent')
            if args.mode=='public':
                browser.wait('!document.getElementById("live-panel").hidden && document.getElementById("live-objects").textContent!=="—"')
                assert 'AS3333' in query('AS3333')
                assert '193.0.6.0/24' in query('193.0.6.0/24')
                assert '2001:67c:2e8::/48' in query('2001:67c:2e8::/48')
                assert '193.0.6.0/24' in query('AS3333','origin')
                assert '2001:' in query('AS3333','origin','6')
                assert '193.0.6.0/24' in query('AS-RIPE','set')
                assert 'No ' in query('RIRRD-NO-SUCH-OBJECT-SECURITY-CHECK')
                assert 'ARIN' not in browser.js('[...document.getElementById("query-source").options].map(o=>o.value)')
                telemetry=browser.js('fetch("live.json").then(r=>r.json())')
                assert telemetry['service']=='online' and telemetry['total_objects']>900000
                assert telemetry['http_queries']>=6
            else:
                assert 'AS64496' in query('AS64496')
                assert '192.0.2.0/24' in query('192.0.2.0/24')
                assert '2001:db8:100::/48' in query('2001:db8:100::/48')
                assert query('AS64496','origin')=='192.0.2.0/24'
                assert query('AS64496','origin','6')=='2001:db8:100::/48'
                assert set(query('AS64496:AS-DOCS','set').splitlines())=={'192.0.2.0/24','198.51.100.0/24'}
                assert set(query('AS64496:AS-DOCS','set','6').splitlines())=={'2001:db8:100::/48','2001:db8:200::/48'}
                assert 'No ' in query('AS64511')
            if args.mode=='live':
                text=query('DOCS-MNT');assert 'DOCS-MNT' in text and 'auth:' not in text and 'DummyValue' not in text
                browser.js('document.getElementById("query-source").add(new Option("UNKNOWN","UNKNOWN"))')
                query('AS64496',source='UNKNOWN')
                assert 'HTTP 400' in browser.js('document.getElementById("query-status").textContent')
            browser.js('location.hash="network"')
            browser.wait('document.getElementById("network-list").textContent.includes("No verified community nodes")')
            assert browser.js('document.getElementById("network-nodes").textContent')=='0'
            for width in [320,390,1440]:
                browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
                for page in ['overview','query','sources','network','connect']:
                    browser.js('location.hash='+json.dumps(page))
                    browser.wait('!document.getElementById("page-'+page+'").hidden')
                    sizes=browser.js('({width:innerWidth,scroll:document.documentElement.scrollWidth})')
                    assert sizes['scroll']<=sizes['width'],(width,page,sizes)
                if args.screenshots:
                    args.screenshots.mkdir(parents=True,exist_ok=True)
                    browser.js('location.hash="overview"');browser.wait('!document.getElementById("page-overview").hidden')
                    shot=browser.call('Page.captureScreenshot',{'format':'png','captureBeyondViewport':False},True)
                    (args.screenshots/f'dashboard-{args.mode}-{width}.png').write_bytes(base64.b64decode(shot['data']))
            requests=[e['params']['request']['url'] for e in browser.events if e.get('method')=='Network.requestWillBeSent']
            assert all(url.startswith(origin) or url=='about:blank' for url in requests),'unexpected external page request'
            if args.mode=='preview':assert not any('/graphql' in url or '/v1/' in url for url in requests),'preview contacted a registry API'
            exceptions=[e for e in browser.events if e.get('method')=='Runtime.exceptionThrown']
            assert not exceptions,'browser JavaScript exception'
            print(args.mode+': object/prefix/origin/set queries, source handling, empty directory, no external requests; all five views fit 320/390/1440px')
        finally:browser.close()

if __name__=='__main__':main()
