'use strict';
(() => {
  const el = id => document.getElementById(id);
  const pages = {overview:'Overview',query:'Registry explorer',sources:'Sources',network:'Community network',connect:'Connect your router'};
  let metadata = null, sample = null, currentResult = '', activeQuery = null, network = [];
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
      text('interface-state',preview?'Preview':'Local API');text('interface-note',preview?'Bundled synthetic data':'Read-only dashboard');
      text('node-name',value.node.name);text('node-location',value.node.location || 'Not configured');text('node-sponsor',value.node.sponsor || 'Not configured');
      text('whois-address',value.whois_listen || 'No public endpoint');
      text('sidebar-mode',preview?'Synthetic preview':'Private HTTP instance');text('version','RIRRd '+value.version);
      text('mode-badge',preview?'Synthetic preview':'Private instance');el('mode-badge').className='badge '+(preview?'amber':'positive');
      notice(preview ? 'Dashboard preview · Bundled synthetic records only. No live registry is connected and no public registry endpoint is deployed here.' : 'Private instance · Read-only queries use this local RIRRd service. Full IRRd compatibility and production qualification remain in development.',preview?'preview':'');
      el('query-source').replaceChildren(new Option('All configured sources',''));
      value.sources.forEach(source=>el('query-source').add(new Option(source,source)));
      el('query-submit').disabled=false;
      if (!preview && value.whois_listen) text('connect-endpoint','Your configured WHOIS listener: '+value.whois_listen+'. Choose your own source and AS-set.');
      await refreshSources();
    } catch (error) { metadata=null;notice('Unable to connect: '+errorText(error),'error');text('mode-badge','Unavailable');el('query-submit').disabled=true; }
  }
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
