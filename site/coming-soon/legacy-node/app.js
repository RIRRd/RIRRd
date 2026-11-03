'use strict';
(() => {
  const el=id=>document.getElementById(id), set=(id,value)=>{el(id).textContent=value;};
  let meta=null, lastResult='', polling=false, request=null;
  const json=async(path,options={})=>{const response=await fetch(path,{cache:'no-store',signal:AbortSignal.timeout(15000),...options});if(!response.ok)throw new Error('HTTP '+response.status);return response.json();};
  const legacy=['performance','compatibility','community','roadmap'];if(legacy.includes(location.hash.slice(1)))location.replace('/project/'+location.hash);
  function queryText(value,source,exact) {
    const scope=source?'-s '+source+' ':'';
    const ip=/^(?:\d{1,3}\.){3}\d{1,3}(?:\/\d{1,2})?$/.test(value) || (value.includes(':') && /^[a-f0-9:]+(?:\/\d{1,3})?$/i.test(value));
    if(exact && !ip)throw new Error('Use an IP address or prefix for exact route lookup.');
    if(ip)return scope+(exact || value.includes('/')?'-x ':'-L ')+(value.includes('/')?value:value+(value.includes(':')?'/128':'/32'));
    return scope+value;
  }
  async function lookup(event) {
    event.preventDefault();if(!meta)return;
    const value=el('value').value.trim(),kind=el('kind').value,source=el('source').value,family=Number(el('family').value);
    if(!value || value.length>256 || !/^[a-z0-9][a-z0-9:._/ -]*$/i.test(value)){set('query-status','Enter a valid IP, prefix, ASN or object key.');return;}
    request?.abort();const controller=new AbortController();request=controller;const timeout=setTimeout(()=>controller.abort(),15000);
    el('submit').disabled=true;el('copy').disabled=true;set('query-status','Querying this RIRRd node…');
    try {
      let q;
      if(['object','exact'].includes(kind)) q=queryText(value,source,kind==='exact');
      else {
        if(kind==='origin' && (!/^AS\d+$/i.test(value) || Number(value.slice(2))>4294967295))throw new Error('Use an ASN such as AS3333.');
        const command=kind==='origin'?(family===6?'!6':'!g'):kind==='members'?'!i':kind==='recursive'?'!i':family===6?'!a6':'!a4';
        q=command+value+(kind==='recursive'?',1':'');
      }
      const parameters=new URLSearchParams({q});if(source)parameters.set('sources',source);
      const response=await fetch('/v1/whois/?'+parameters,{cache:'no-store',signal:controller.signal});
      if(!response.ok)throw new Error('WHOIS returned HTTP '+response.status);
      let result=response.status===204?'':await response.text();
      if(result.length>1048576)throw new Error('Result exceeds the display limit. Narrow the query.');
      lastResult=result;set('result',result || 'No matching records.');set('query-status',result?'Result from '+(source || 'configured web source order')+' · read-only query':'No matching records in the selected scope.');el('copy').disabled=!result;
    } catch(error){lastResult='';set('result','No usable response was returned.');set('query-status',error.name==='AbortError'?'Query cancelled or timed out.':String(error.message).slice(0,500));}
    finally{clearTimeout(timeout);el('submit').disabled=false;request=null;}
  }
  async function refresh() {
    if(polling)return;polling=true;
    try {
      if(!meta){meta=await json('/dashboard/meta.json');set('node-name',meta.node.name);el('submit').disabled=false;}
      const live=await json('/dashboard/live.json');
      set('node-status',live.service==='online'?'IRR online · Experimental':'IRR status unavailable');el('node-status').className='pill '+(live.service==='online'?'':'amber');
      const sources=live.sources || [],loaded=sources.filter(s=>s.state==='loaded');
      set('objects',Number(live.total_objects || 0).toLocaleString());set('sources',loaded.length+' / '+sources.length);
      const following=loaded.filter(s=>s.mirror?.state==='following' && Date.now()/1000-s.mirror.checked_at_unix_seconds<Math.max(180,s.mirror.interval_seconds*3));
      set('mirrors',following.length);set('latency',live.http_recent_p95_ms==null?'No sample':Number(live.http_recent_p95_ms).toFixed(2)+' ms');
      set('observed','Observed '+live.observed_at+' · Counters exclude TCP queries. Full IRRd/Routinator parity and production qualification remain incomplete.');
      const selected=el('source').value;el('source').replaceChildren(new Option('All loaded web sources',''));
      loaded.forEach(s=>el('source').add(new Option(s.source,s.source)));if([...el('source').options].some(o=>o.value===selected))el('source').value=selected;
      el('source-list').replaceChildren();for(const source of sources){const chip=document.createElement('button');chip.type='button';chip.className='source-chip'+(source.state==='loaded'?'':' pending');const title=document.createElement('strong'),detail=document.createElement('small');title.textContent=source.source;detail.textContent=source.state!=='loaded'?'Feed unavailable':source.mirror?.state==='following'?'NRTM following':source.mirror?'Mirror needs attention':'Daily snapshot';chip.append(title,detail);chip.addEventListener('click',()=>{if(source.state==='loaded'){el('source').value=source.source;el('value').focus();location.hash='query';}else location.href='/dashboard/#sources';});el('source-list').append(chip);}
      const totals={};for(const counts of Object.values(live.class_counts || {}))for(const [name,total] of Object.entries(counts))totals[name]=(totals[name] || 0)+total;
      el('class-stats').replaceChildren();for(const [name,label] of [['route','IPv4 route objects'],['route6','IPv6 route objects'],['as-set','AS-sets'],['aut-num','AS number objects'],['route-set','Route sets']]){const item=document.createElement('article'),title=document.createElement('span'),total=document.createElement('strong');title.textContent=label;total.textContent=live.class_counts?Number(totals[name] || 0).toLocaleString():'Awaiting live index';item.append(title,total);el('class-stats').append(item);}
      const browseSelected=el('browse-source').value;el('browse-source').replaceChildren();loaded.filter(s=>s.http_access!==false).forEach(s=>el('browse-source').add(new Option(s.source,s.source)));if([...el('browse-source').options].some(o=>o.value===browseSelected))el('browse-source').value=browseSelected;
      set('source-note',following.length+' incremental feeds, '+(loaded.length-loaded.filter(s=>s.mirror || s.mirror_bootstrap).length)+' daily snapshot feeds, '+(sources.length-loaded.length)+' unavailable feeds. Last-check success is not a guarantee of zero upstream delay. NRTMv3 transport is unauthenticated.');
    }catch(error){set('node-status','Service status unavailable');el('node-status').className='pill amber';set('observed','Current telemetry is unavailable; previously shown figures may be stale.');}
    finally{polling=false;}
    await rpki();
  }
  async function rpki() {
    try {
      const data=await json('/v1/rpki/status');
      set('rpki-state',data.usable?'Validated dataset available':data.enabled?'Validator starting / unavailable':'Validator not enabled');el('rpki-state').className='pill '+(data.usable?'':'amber');
      set('rpki-detail',data.message || 'Embedded RPKI engine status is available.');
      el('rpki-stats').replaceChildren();for(const [label,value] of [['Validated VRPs',data.vrps],['Serial',data.serial],['Updated',data.last_success?new Date(data.last_success*1000).toISOString():null],['RTR',data.rtr_listen || 'Not configured']]){const item=document.createElement('span');item.textContent=label+': '+(value ?? 'Unreported');el('rpki-stats').append(item);}
      el('rpki-submit').disabled=!data.usable;
    }catch{set('rpki-state','Validator integration in development');set('rpki-detail','This running release does not yet expose the integrated validator. IRR queries above are live; RPKI trust-chain validation and RTR are separate capabilities under development.');el('rpki-submit').disabled=true;}
  }
  let cursor=null;
  async function browse(next=false){el('browse-submit').disabled=true;try{const params=new URLSearchParams({source:el('browse-source').value,class:el('browse-class').value,limit:30});if(next && cursor)params.set('after',cursor);const data=await json('/v1/irr/objects?'+params);cursor=data.next_cursor;el('browse-next').disabled=!cursor;el('browse-results').replaceChildren();for(const key of data.keys){const button=document.createElement('button');button.type='button';button.className='quiet';button.textContent=key;button.addEventListener('click',()=>{el('value').value=key.split('AS')[0] && ['route','route6'].includes(data.object_class)?key.split('AS')[0]:key;el('kind').value='object';el('source').value=data.source;el('family').disabled=true;el('lookup').requestSubmit();location.hash='query';});el('browse-results').append(button);}set('browse-status',data.keys.length+' keys shown · '+data.source+' / '+data.object_class+' · live index; data may change between pages.');}catch(error){set('browse-status','Browse unavailable: '+error.message);}finally{el('browse-submit').disabled=false;}}
  el('browse-form').addEventListener('submit',event=>{event.preventDefault();cursor=null;browse();});el('browse-next').addEventListener('click',()=>browse(true));
  el('lookup').addEventListener('submit',lookup);el('family').disabled=true;el('kind').addEventListener('change',()=>{el('family').disabled=!['origin','set'].includes(el('kind').value);});
  document.querySelectorAll('[data-example]').forEach(button=>button.addEventListener('click',()=>{el('value').value=button.dataset.example;el('kind').value='object';el('family').disabled=true;el('value').focus();}));
  el('copy').addEventListener('click',async()=>{try{await navigator.clipboard.writeText(lastResult);set('copy','Copied');setTimeout(()=>set('copy','Copy results'),1500);}catch{set('query-status','Select and copy the displayed result.');}});
  el('rpki-form').addEventListener('submit',async event=>{event.preventDefault();const prefix=el('rpki-prefix').value.trim(),origin=el('rpki-origin').value.trim();el('rpki-submit').disabled=true;try{const data=await json('/v1/rpki/validate?'+new URLSearchParams({prefix,origin}));set('rpki-result',JSON.stringify(data,null,2));}catch(error){set('rpki-result','Validation unavailable: '+error.message);}finally{await rpki();}});
  refresh();setInterval(()=>{if(!document.hidden)refresh();},15000);
})();
