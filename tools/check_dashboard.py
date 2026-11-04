#!/usr/bin/env python3
"""Exercise the real read-only dashboard in Chromium over DevTools pipes."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import signal
import sys
import shlex
import shutil
import subprocess
import tempfile
import time
from urllib.parse import urlsplit


class Browser:
    def __init__(self, executable, profile, extra_args=()):
        command=[executable,'--headless','--no-sandbox','--disable-gpu','--disable-background-networking','--no-first-run','--remote-debugging-pipe','--user-data-dir='+profile, *extra_args]
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
    parser=argparse.ArgumentParser(description='Check the independent bundled RIRRd console')
    parser.add_argument('--url',required=True)
    parser.add_argument('--mode',choices=['live','public'],required=True)
    parser.add_argument('--browser',default=shutil.which('chromium') or shutil.which('google-chrome'))
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    if args.mode=='public':
        import subprocess
        origin=urlsplit(args.url)
        command=[sys.executable,str(Path(__file__).with_name('check_community_ui.py')),'--url',origin.scheme+'://'+origin.netloc]
        if args.screenshots:command += ['--screenshots',str(args.screenshots)]
        subprocess.run(command,check=True)
        return
    if not args.browser:parser.error('Chromium or Google Chrome is required')
    with tempfile.TemporaryDirectory(prefix='rirrd-console-browser-') as profile:
        browser=Browser(args.browser,profile)
        try:
            browser.call('Page.navigate',{'url':args.url},True)
            browser.wait('document.getElementById("query-submit") && !document.getElementById("query-submit").disabled')
            assert browser.js('document.getElementById("mode-badge").textContent')=='Private instance'
            assert not browser.js("document.querySelector('[data-page=network]') !== null")
            assert not browser.js('document.body.textContent.includes("HYEHOST LLC")')
            assert browser.js('document.getElementById("community-link").hidden')
            def query(value,kind='object',family='4',source='EXAMPLE'):
                browser.js('location.hash="query";document.getElementById("query-value").value='+json.dumps(value)+';document.getElementById("query-kind").value='+json.dumps(kind)+';document.getElementById("query-family").value='+json.dumps(family)+';document.getElementById("query-source").value='+json.dumps(source)+';document.getElementById("query-form").requestSubmit()')
                browser.wait('!document.getElementById("query-submit").disabled')
                return browser.js('document.getElementById("query-result").textContent')
            assert 'AS64496' in query('AS64496')
            assert '192.0.2.0/24' in query('192.0.2.0/24')
            assert '192.0.2.0/24' in query('192.0.2.1','covering')
            assert '192.0.2.0/24' in query('192.0.2.0/23','more')
            assert '2001:db8:100::/48' in query('2001:db8:100::/48')
            assert query('AS64496','origin')=='192.0.2.0/24'
            assert query('AS64496','origin','6')=='2001:db8:100::/48'
            assert set(query('AS64496:AS-DOCS','set').splitlines())=={'192.0.2.0/24','198.51.100.0/24'}
            assert set(query('AS64496:AS-DOCS','set','6').splitlines())=={'2001:db8:100::/48','2001:db8:200::/48'}
            assert set(query('AS64496:AS-DOCS','recursive').splitlines())=={'AS64496','AS64497'}
            assert 'No matching' in query('AS64511')
            result=query('DOCS-MNT');assert 'DOCS-MNT' in result and 'auth:' not in result and 'DummyValue' not in result
            browser.js('document.getElementById("query-source").add(new Option("UNKNOWN","UNKNOWN"))')
            query('AS64496',source='UNKNOWN');assert 'HTTP 400' in browser.js('document.getElementById("query-status").textContent')
            browser.js('location.hash="api";document.getElementById("load-schema").click()');browser.wait('!document.getElementById("load-schema").disabled')
            assert 'queryType' in browser.js('document.getElementById("schema-result").textContent')
            browser.js('document.getElementById("graphql-form").requestSubmit()');browser.wait('!document.getElementById("graphql-submit").disabled')
            assert 'databaseStatus' in browser.js('document.getElementById("graphql-result").textContent')
            assert browser.js('document.querySelectorAll("#command-examples .command-card").length')==6
            assert '!6AS64496' in browser.js('document.getElementById("command-examples").textContent')
            for width in [320,390,1440]:
                browser.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
                for page in ['overview','query','sources','rpki','api','connect','system']:
                    browser.js('location.hash='+json.dumps(page));browser.wait('!document.getElementById("page-'+page+'").hidden')
                    assert browser.js('document.documentElement.scrollWidth<=innerWidth'),(width,page)
            assert not [event for event in browser.events if event.get('method')=='Runtime.exceptionThrown']
            print('Independent self-hosted console: no community directory/hosting data, real object/prefix/v4/v6/set/member queries, credential redaction, schema/workbench and seven responsive views passed')
        finally:browser.close()


if __name__=='__main__':main()
