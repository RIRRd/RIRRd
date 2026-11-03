#!/usr/bin/env python3
"""Exercise the public node landing page against its real same-origin APIs."""
import argparse,json,shutil,tempfile
from pathlib import Path
from check_dashboard import Browser
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--url',required=True);p.add_argument('--screenshots',type=Path);a=p.parse_args()
with tempfile.TemporaryDirectory(prefix='rirrd-node-browser-') as profile:
 b=Browser(shutil.which('chromium') or shutil.which('google-chrome'),profile)
 try:
  b.call('Page.navigate',{'url':a.url},True);b.wait('document.getElementById("submit") && !document.getElementById("submit").disabled && document.getElementById("sources").textContent!=="—"')
  assert b.js('document.body.textContent.includes("HYEHOST LLC") && document.body.textContent.includes("United Kingdom")')
  assert 'ARIN' not in b.js('[...document.getElementById("source").options].map(o=>o.value)')
  def query(value,kind='object',family='4'):
   b.js('document.getElementById("value").value='+json.dumps(value)+';document.getElementById("source").value="RIPE";document.getElementById("kind").value='+json.dumps(kind)+';document.getElementById("family").value='+json.dumps(family)+';document.getElementById("lookup").requestSubmit()')
   b.wait('!document.getElementById("submit").disabled')
   return b.js('document.getElementById("result").textContent')
  assert 'AS3333' in query('AS3333')
  assert '193.0.0.0/21' in query('193.0.0.1')
  assert '193.0.0.0/21' in query('193.0.0.0/21','exact')
  assert '193.0.0.0/21' in query('AS3333','origin')
  assert '2001:' in query('AS3333','origin','6')
  assert '193.0.0.0/21' in query('AS-RIPENCC','set')
  assert 'No matching' in query('RIRRD-ABSENT-OBJECT-TEST')
  for width in [320,390,1440]:
   b.call('Emulation.setDeviceMetricsOverride',{'width':width,'height':1100,'deviceScaleFactor':1,'mobile':width<650},True)
   assert b.js('document.documentElement.scrollWidth<=innerWidth'),width
   if a.screenshots:
    import base64
    a.screenshots.mkdir(exist_ok=True,parents=True);b.js('scrollTo(0,0)');shot=b.call('Page.captureScreenshot',{'format':'png'},True);(a.screenshots/f'node-{width}.png').write_bytes(base64.b64decode(shot['data']))
  assert not [e for e in b.events if e.get('method')=='Runtime.exceptionThrown']
  print('UK/HYEHOST node portal: object/IP/prefix/origin/set/empty queries, ARIN isolation, responsive 320/390/1440px')
 finally:b.close()
