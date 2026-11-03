#!/usr/bin/env python3
"""Check chart reuse, inspection, resize, gaps and mobile navigation in Chromium."""
import argparse
import shutil
import tempfile
from check_dashboard import Browser
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--project',required=True);parser.add_argument('--community',required=True)
args=parser.parse_args()
with tempfile.TemporaryDirectory(prefix='rirrd-chart-check-') as profile:
    browser=Browser(shutil.which('chromium') or shutil.which('google-chrome'),profile)
    try:
        for url in [args.project.rstrip('/')+'/',args.project.rstrip('/')+'/docs/beta/',args.community.rstrip('/')+'/',args.community.rstrip('/')+'/guides/api/']:
            browser.call('Emulation.setDeviceMetricsOverride',{'width':390,'height':1000,'deviceScaleFactor':1,'mobile':True},True)
            browser.call('Page.navigate',{'url':url},True)
            browser.wait('document.getElementById("site-navigation")?.hidden===true')
            browser.js('document.querySelector(".menu-toggle").click()')
            assert browser.js('!document.getElementById("site-navigation").hidden && document.querySelector(".menu-toggle").getAttribute("aria-expanded")==="true"')
            browser.js('document.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape"}))')
            assert browser.js('document.getElementById("site-navigation").hidden')
            assert browser.js('document.documentElement.scrollWidth<=innerWidth')
        browser.call('Emulation.setDeviceMetricsOverride',{'width':1440,'height':1100,'deviceScaleFactor':1,'mobile':False},True)
        browser.call('Page.navigate',{'url':args.community.rstrip('/')+'/dashboard/'},True)
        browser.wait('document.getElementById("source-chart")?.dataset.points==="19"')
        browser.wait('document.getElementById("history-note").textContent.includes("Collection began")')
        browser.js('''window.testHost=document.getElementById('query-chart');window.testPoints=[{timestamp:1000,value:1},{timestamp:1060,value:2},{timestamp:1120,value:null},{timestamp:1360,value:4}];RIRRdCharts.line(testHost,testPoints,'value','Chart interaction fixture');window.originalSvg=testHost.querySelector('svg');''')
        browser.js("RIRRdCharts.line(testHost,testPoints.map(p=>({...p})),'value','Chart interaction fixture')")
        assert browser.js("originalSvg===testHost.querySelector('svg')")
        assert browser.js("testHost.querySelectorAll('polyline').length") == 2
        browser.js("const input=testHost.querySelector('input');input.value='0';input.focus();input.dispatchEvent(new Event('input'));testPoints.push({timestamp:1420,value:5});RIRRdCharts.line(testHost,testPoints,'value','Chart interaction fixture')")
        assert browser.js("testHost.querySelector('input').value")=='0'
        assert browser.js("document.activeElement===testHost.querySelector('input')")
        assert 'Selected: 1' in browser.js("testHost.querySelector('.chart-readout').textContent")
        before=browser.js("testHost.querySelector('svg').viewBox.baseVal.width")
        browser.call('Emulation.setDeviceMetricsOverride',{'width':320,'height':1000,'deviceScaleFactor':1,'mobile':True},True)
        browser.wait("testHost.querySelector('svg').viewBox.baseVal.width<"+str(before))
        assert browser.js("testHost.querySelector('input').value")=='0'
        browser.js("testHost.querySelector('.chart-latest').click()")
        assert browser.js("testHost.querySelector('input').value")=='3'
        assert browser.js('document.documentElement.scrollWidth<=innerWidth')
        browser.js('location.hash="sources"')
        browser.wait('!document.getElementById("page-sources").hidden')
        browser.js('document.getElementById("source-detail").value="RIPE";document.getElementById("source-detail").dispatchEvent(new Event("change"));document.querySelector("#source-detail-content details").open=true')
        browser.js('''new Promise((resolve,reject)=>{const target=document.getElementById('source-detail-content');const timeout=setTimeout(()=>{observer.disconnect();reject(Error('No refresh'));},10000);const observer=new MutationObserver(()=>{observer.disconnect();clearTimeout(timeout);resolve(true)});observer.observe(target,{childList:true});document.getElementById('refresh').click();})''')
        assert browser.js('document.querySelector("#source-detail-content details").open')
        assert not [event for event in browser.events if event.get('method')=='Runtime.exceptionThrown']
        print('Mobile menus on both sites/docs; unchanged SVG reuse; pinned sample and keyboard focus; responsive redraw; missing-data gaps; latest control; open source receipts survive refresh')
    finally:browser.close()
