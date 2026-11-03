'use strict';
(() => {
  const el = id => document.getElementById(id);
  const pages = {overview:'Overview',query:'Registry explorer',sources:'Sources',network:'Community network',connect:'Connect your router'};
  let metadata = null, sample = null, currentResult = '', activeQuery = null, network = [], refreshingLive = false;
  const text = (id, value) => { el(id).textContent = value; };
  const errorText = error => String(error.message || error).slice(0, 700);
  function page() {
    const key = location.hash.slice(1) || 'overview';
    const selected = Object.hasOwn(pages, key) ? key : 'overview';
    Object.keys(pages).forEach(name => { el('page-'+name).hidden = name !== selected; });
    document.querySelectorAll('nav [data-page]').forEach(link => {
      if (link.dataset.page === selected) link.setAttribute('aria-current','page');
      else link.removeAttribute('aria-current');
    });
    text('breadcrumb',pages[selected]);
    document.title = `RIRRd · ${pages[selected]}`;
  }
  async function json(url, options = {}) {
    const response = await fetch(url, {cache:'no-store',...options});
    if (!response.ok) throw new Error(`Service returned HTTP ${response.status}.`);
    const value = await response.json();
    if (value.errors?.length) throw new Error(value.errors.map(e=>e.message).join('; '));
    return value;
  }
  async function graphql(query, variables, signal) {
    return json('/graphql/',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({query,variables}),signal});
  }
  function notice(message, kind) { text('mode-notice',message);el('mode-notice').className='notice '+kind; }
  function cell(row, value, tag='td') {
    const item=document.createElement(tag);item.textContent=value;
    if (tag==='th') item.scope='row';
    row.append(item);
  }
  function renderSources(sources) {
    el('source-rows').replaceChildren();el('overview-sources').replaceChildren();
    for (const source of sources) {
      const role=source.authoritative ? 'Authoritative' : 'Mirror / snapshot';
      const row=document.createElement('tr');
      cell(row,source.source,'th');cell(row,role);
      cell(row,source.serialNewestMirror ?? 'Untracked');cell(row,source.nrtm4ClientVersion ?? 'Not configured');
      cell(row,source.lastUpdate || source.rpslDataUpdated || 'Not reported');el('source-rows').append(row);
      const pill=document.createElement('div');pill.className='source-pill';
      const title=document.createElement('strong');title.textContent=source.source;
      const detail=document.createElement('span');detail.textContent=role+' · freshness unverified';
      pill.append(title,detail);el('overview-sources').append(pill);
    }
    if (!sources.length) {const p=document.createElement('p');p.className='empty';p.textContent='No source status was returned.';el('overview-sources').append(p);}
    text('sources-state',(metadata.mode==='preview' ? 'Synthetic source status. ' : 'Status fetched at '+new Date().toLocaleTimeString()+'. ')+'A cursor does not establish source freshness.');
  }
  async function refreshSources() {
    if (!metadata) return;
    if (metadata.public_service) return refreshLive();
    el('refresh').disabled=el('refresh-sources').disabled=true;
    try {
      const rows=metadata.mode==='preview' ? sample.sources : (await graphql('{ databaseStatus { source authoritative serialNewestMirror nrtm4ClientVersion lastUpdate rpslDataUpdated } }',{})).data.databaseStatus;
      renderSources(rows);
    } catch (error) { text('sources-state','Status unavailable: '+errorText(error));text('interface-state','Error');text('interface-note','Source status unavailable'); }
    finally { el('refresh').disabled=el('refresh-sources').disabled=false; }
  }
  async function initialize() {
    try {
      const value=await json('meta.json');
      if (!['live','preview'].includes(value.mode) || !Array.isArray(value.sources)) throw new Error('Invalid dashboard metadata.');
      metadata=value;
      const preview=value.mode==='preview';
      if (preview) sample=await json('sample-results.json');
      if (preview) {text('connection-label','Example connection cap');text('cache-label','Example cache budget');}
      text('source-count',value.sources.length);text('connection-cap',value.max_connections ?? '—');
      text('cache-budget',value.database_cache_mib ? value.database_cache_mib+' MiB' : '—');
      text('interface-state',preview?'Preview':value.public_service?'Live API':'Local API');text('interface-note',preview?'Bundled synthetic data':'Read-only dashboard');
      text('node-name',value.node.name);text('node-location',value.node.location || 'Not configured');text('node-sponsor',value.node.sponsor || 'Not configured');
      text('whois-address',value.whois_listen || 'No public endpoint');
      text('sidebar-mode',preview?'Synthetic preview':value.public_service?'Public test registry':'Private HTTP instance');text('version','RIRRd '+value.version);
      text('mode-badge',preview?'Synthetic preview':value.public_service?'Live · Experimental':'Private instance');el('mode-badge').className='badge '+(preview?'amber':'positive');
      if (value.public_service) {
        el('source-count').previousElementSibling.textContent='Loaded / tracked sources';
        text('connection-label','WHOIS active / cap');el('connection-cap').nextElementSibling.textContent='Current sockets / global limit';
        text('quick-example','AS3333');el('query-value').placeholder='AS3333 or 193.0.0.0/21';
        const replacements={'AS64496':'AS3333','192.0.2.0/24':'193.0.0.0/21','2001:db8:100::/48':'2001:67c:2e8::/48','AS64496:AS-DOCS':'AS-RIPENCC'};
        document.querySelectorAll('[data-example]').forEach(button=>{const next=replacements[button.dataset.example];if(next){if(button.textContent===button.dataset.example)button.textContent=next;button.dataset.example=next;}});
        text('connect-command','bgpq4 -4 -b -h rirrd.co.uk:43 -S RIPE -l RIPE_TEST AS3333');
        text('whois-address','rirrd.co.uk:43');
      }
      notice(value.public_service ? 'Live experimental IRR · Read-only queries run on this RIRRd instance using real routing snapshots. Check source coverage and timestamps before testing router policy. Full parity, live RPKI filtering and production qualification remain incomplete.' : preview ? 'Dashboard preview · Bundled synthetic records only. No live registry is connected and no public registry endpoint is deployed here.' : 'Private instance · Read-only queries use this local RIRRd service. Full IRRd compatibility and production qualification remain in development.',preview?'preview':'');
      el('query-source').replaceChildren(new Option('All configured sources',''));
      (value.http_sources || value.sources).forEach(source=>el('query-source').add(new Option(source,source)));
      el('query-submit').disabled=false;
      if (!preview && value.whois_listen) text('connect-endpoint','Your configured WHOIS listener: '+value.whois_listen+'. Choose your own source and AS-set.');
      await refreshSources();
    } catch (error) { metadata=null;notice('Unable to connect: '+errorText(error),'error');text('mode-badge','Unavailable');el('query-submit').disabled=true; }
  }
  async function refreshLive() {
    if (refreshingLive || !metadata?.public_service) return;
    refreshingLive=true;
    try {
      const live=await json('live.json',{signal:AbortSignal.timeout(12000)});
      el('live-panel').hidden=false;
      const mib=bytes=>Number.isFinite(bytes)?(bytes/1048576).toFixed(1)+' MiB':'—';
      text('connection-cap',(live.whois_connections ?? '—')+' / '+metadata.max_connections);
      text('live-objects',Number(live.total_objects || 0).toLocaleString());
      text('live-rss',mib(live.process_rss_bytes));text('live-memory',mib(live.cgroup_memory_bytes));text('live-disk',mib(live.database_bytes));
      text('live-cpu',live.cpu_percent_one_core == null?'—':Number(live.cpu_percent_one_core).toFixed(1)+'%');
      const hours=Math.floor((live.uptime_seconds || 0)/3600),minutes=Math.floor((live.uptime_seconds || 0)%3600/60);
      text('live-uptime',hours+'h '+minutes+'m');text('live-queries',Number(live.http_queries || 0).toLocaleString());
      text('live-errors',(live.http_errors || 0)+' HTTP error responses · since gateway restart');
      text('live-latency',live.http_recent_p95_ms == null?'No sample':Number(live.http_recent_p95_ms).toFixed(2)+' ms');
      text('live-observed','Observed '+(live.observed_at || 'unavailable')+' · '+(live.refresh_mode || 'refresh unreported'));
      text('interface-state',live.service==='online'?'Live API':'Unavailable');
      text('interface-note',live.service==='online'?'Public read-only test service':'Telemetry unavailable');
      const rows=live.sources || [],loaded=rows.filter(row=>row.state==='loaded');
      text('source-count',loaded.length+' / '+rows.length);
      el('source-rows').replaceChildren();el('overview-sources').replaceChildren();
      const head=el('source-rows').closest('table').querySelector('thead tr');head.replaceChildren();
      ['Source','State','Objects','Upstream snapshot','Imported locally','Scope / remaining action'].forEach(label=>{const th=document.createElement('th');th.scope='col';th.textContent=label;head.append(th);});
      for(const source of rows){
        const row=document.createElement('tr');cell(row,source.source,'th');const age=source.imported_at?(Date.now()-Date.parse(source.imported_at))/3600000:null;cell(row,source.state==='loaded'?(age>48?'Stale snapshot':source.http_access===false?'Snapshot · TCP only':'Daily snapshot'):source.state);
        cell(row,source.objects==null?'—':Number(source.objects).toLocaleString());
        const modified=[...new Set((source.downloads || []).map(file=>file.last_modified).filter(Boolean))];
        cell(row,modified.join(' / ') || 'Not fetched');cell(row,source.imported_at || 'Not imported');
        cell(row,source.state==='loaded'?Object.keys(source.classes || {}).join(', ')+(source.http_access===false?' · TCP WHOIS only; excluded from web queries.':'')+(source.skipped_lookup_values?' · '+source.skipped_lookup_values+' malformed lookup values not indexed.':'')+(source.duplicate_identities_last_wins?' · '+source.duplicate_identities_last_wins+' duplicate identities collapsed (last record).':''):source.reason || 'Not loaded');
        if(source.permission?.startsWith('https://')){const policy=document.createElement('a');policy.href=source.permission;policy.textContent='Source policy / access';row.lastElementChild.append(document.createElement('br'),policy);}
        el('source-rows').append(row);
        const pill=document.createElement('div');pill.className='source-pill';const title=document.createElement('strong');title.textContent=source.source;
        const detail=document.createElement('span');detail.textContent=source.state==='loaded'?Number(source.objects).toLocaleString()+' objects · snapshot':source.reason || source.state;
        pill.append(title,detail);
        if(source.state==='loaded'){const share=document.createElement('progress');share.max=live.total_objects || 1;share.value=source.objects;share.setAttribute('aria-label',source.source+' share of loaded objects');pill.append(share);el('overview-sources').append(pill);}
      }
      const current=el('query-source').value;
      el('query-source').replaceChildren(new Option('All loaded sources',''));loaded.filter(row=>row.http_access!==false).forEach(row=>el('query-source').add(new Option(row.source,row.source)));
      if(loaded.some(row=>row.source===current))el('query-source').value=current;
      text('sources-state',(live.last_refresh?.state==='failed'?'Refresh failed; last verified generation retained. ':'')+loaded.length+' loaded / '+rows.length+' tracked. Only loaded sources can be queried. ARIN is available through TCP WHOIS; web queries exclude it under its distribution terms. Upstream snapshot time and local import time differ. No near-real-time claim.');
    } catch(error) {
      text('live-observed','Live telemetry unavailable: '+errorText(error));text('interface-state','Unavailable');
      text('sources-state','Source status unavailable; previously displayed timestamps may be stale.');
    } finally {refreshingLive=false;}
  }
  setInterval(()=>{if(!document.hidden && metadata?.public_service)refreshLive();},15000);
  function objectQuery(value, source) {
    const ipv4=/^(?:\d{1,3}\.){3}\d{1,3}(?:\/\d{1,2})?$/.test(value);
    const ipv6=value.includes(':') && /^[0-9a-f:]+(?:\/\d{1,3})?$/i.test(value);
    const scope=source ? '-s '+source+' ' : '';
    if (ipv4 || ipv6) return scope+'-x '+(value.includes('/')?value:value+(ipv6?'/128':'/32'));
    return scope+value;
  }
  async function query(event) {
    event.preventDefault();if (!metadata) return;
    const value=el('query-value').value.trim();const kind=el('query-kind').value;
    const source=el('query-source').value;const family=Number(el('query-family').value);
    if (!value || value.length>256 || !/^[A-Za-z0-9][A-Za-z0-9:._/ -]*$/.test(value)) {
      text('query-status','Enter an ASN, IP prefix or RPSL key (at most 256 characters).');el('query-status').className='result-status error';return;
    }
    if (activeQuery) activeQuery.abort();
    activeQuery=new AbortController();const controller=activeQuery;
    const timeout=setTimeout(()=>controller.abort(),15000);
    el('query-submit').disabled=true;el('copy-result').disabled=true;
    text('query-status','Querying…');el('query-status').className='result-status';
    const start=performance.now();
    try {
      let result='';
      if (metadata.mode==='preview') {
        if (kind==='object') result=sample.objects[value.toUpperCase()] || '';
        else result=((kind==='origin' ? sample.origins : sample.sets)[value.toUpperCase()]?.[String(family)] || []).join('\n');
      } else if (kind==='object') {
        const response=await fetch('/v1/whois/?'+new URLSearchParams({q:objectQuery(value,source)}),{signal:controller.signal,cache:'no-store'});
        if (!response.ok) throw new Error('WHOIS returned HTTP '+response.status+'.');
        result=response.status===204 ? '' : await response.text();
      } else if (kind==='origin') {
        if (!/^AS\d+$/i.test(value) || Number(value.slice(2))>4294967295) throw new Error('Use an origin ASN such as AS64496.');
        const data=await graphql('query($asns:[ASN!]!,$family:Int,$sources:[String!]) { asnPrefixes(asns:$asns,ipVersion:$family,sources:$sources) { asn prefixes } }',{asns:[Number(value.slice(2))],family,sources:source?[source]:null},controller.signal);
        result=data.data.asnPrefixes.flatMap(row=>row.prefixes).join('\n');
      } else {
        const data=await graphql('query($sets:[String!]!,$family:Int,$sources:[String!]) { asSetPrefixes(setNames:$sets,ipVersion:$family,sources:$sources) { rpslPk prefixes } }',{sets:[value],family,sources:source?[source]:null},controller.signal);
        result=data.data.asSetPrefixes.flatMap(row=>row.prefixes).join('\n');
      }
      if (result.length>1048576) throw new Error('Response exceeds the dashboard display limit.');
      if (activeQuery!==controller) return;
      currentResult=result;
      text('query-result',result || (metadata.mode==='preview'?'No bundled example matches this query. The preview only contains its labeled synthetic fixture.':'No matching records.'));
      text('query-status',result ? (metadata.mode==='preview'?'Synthetic example returned':'Query completed') : 'No result');
      text('result-meta',(source || 'Configured source order')+' · '+kind+' query · '+(performance.now()-start).toFixed(0)+' ms'+(metadata.mode==='preview'?' browser preview time':''));
      el('copy-result').disabled=!result;
    } catch (error) {if (activeQuery!==controller) return;currentResult='';text('query-status',error.name==='AbortError'?'Query cancelled or timed out.':errorText(error));el('query-status').className='result-status error';text('query-result','The request did not return a usable result.');}
    finally {clearTimeout(timeout);if (activeQuery===controller) {el('query-submit').disabled=false;activeQuery=null;}}
  }
  async function directory() {
    try {
      const data=await json('network.json');
      if (!Array.isArray(data.nodes) || data.nodes.length>256) throw new Error('Invalid directory.');
      network=data.nodes.filter(n=>n.status==='verified' && n.verified_at);
      const regions=[...new Set(network.map(n=>n.region))].filter(Boolean).sort();
      regions.forEach(region=>el('region-filter').add(new Option(region,region)));
      text('network-nodes',network.length);text('network-regions',regions.length);
      text('network-sponsors',new Set(network.map(n=>n.sponsor).filter(Boolean)).size);
      renderDirectory();
    } catch (error) {text('network-list','Directory unavailable: '+errorText(error));}
  }
  function renderDirectory() {
    const rows=network.filter(node=>!el('region-filter').value || node.region===el('region-filter').value);
    el('network-list').replaceChildren();
    if (!rows.length) {
      const box=document.createElement('div');box.className='directory-empty';
      const title=document.createElement('h3');title.textContent=network.length?'No listings in this region':'No verified community nodes listed yet';
      const copy=document.createElement('p');copy.textContent=network.length?'Choose another region to view the available listings.':'The directory is planned. Locations, sponsors, endpoints and verification dates will appear after operator consent and service checks.';
      box.append(title,copy);el('network-list').append(box);return;
    }
    for (const node of rows) {
      const card=document.createElement('article');card.className='directory-node';
      const title=document.createElement('h3');title.textContent=node.name;
      const detail=document.createElement('p');detail.textContent=[node.city,node.country,node.sponsor?'Sponsor: '+node.sponsor:null].filter(Boolean).join(' · ');
      const endpoint=document.createElement('code');endpoint.textContent=node.whois || 'No WHOIS endpoint listed';
      const verified=document.createElement('p');verified.textContent='Verified: '+node.verified_at+' · '+(node.sources || []).join(', ');
      card.append(title,detail,endpoint,verified);el('network-list').append(card);
    }
  }
  window.addEventListener('hashchange',page);page();
  document.querySelectorAll('[data-example]').forEach(button=>button.addEventListener('click',()=>{el('query-value').value=button.dataset.example;el('query-kind').value='object';el('query-family').disabled=true;location.hash='query';el('query-value').focus();}));
  el('query-kind').addEventListener('change',()=>{el('query-family').disabled=el('query-kind').value==='object';});el('query-family').disabled=true;
  el('query-form').addEventListener('submit',query);
  el('refresh').addEventListener('click',()=>metadata?refreshSources():initialize());el('refresh-sources').addEventListener('click',refreshSources);
  el('region-filter').addEventListener('change',renderDirectory);
  el('copy-result').addEventListener('click',async()=>{try {await navigator.clipboard.writeText(currentResult);text('copy-result','Copied');setTimeout(()=>text('copy-result','Copy result'),1600);}catch {text('query-status','Clipboard unavailable. Select and copy the result text.');}});
  initialize();directory();
})();
