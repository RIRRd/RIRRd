'use strict';
(() => {
  const el = id => document.getElementById(id);
  const legacy = {query:'/dashboard/#query',rpki:'/dashboard/#rpki',sources:'/dashboard/#sources',performance:'/project/',compatibility:'/self-host/',network:'/#nodes',community:'/#nodes'};
  if (legacy[location.hash.slice(1)]) location.replace(legacy[location.hash.slice(1)]);
  let directory = [], loading = false;
  const number = value => Number(value || 0).toLocaleString();
  function render() {
    const selected = el('region').value;
    el('node-list').replaceChildren();
    const nodes = directory.filter(node => !selected || node.region === selected);
    if (!nodes.length) { const p = document.createElement('p'); p.textContent = 'No reviewed node in this region yet. You can submit one below.'; el('node-list').append(p); }
    for (const node of nodes) {
      const card = document.createElement('article'); card.className = 'node-card';
      const top = document.createElement('div'); top.className = 'node-top';
      const place = document.createElement('span'), status = document.createElement('span'); place.textContent = node.country || node.location; status.className = 'tag'; status.textContent = node.experimental ? 'Experimental public node' : 'Reviewed listing'; top.append(place, status);
      const title = document.createElement('h3'); title.textContent = node.name;
      const intro = document.createElement('p'); intro.textContent = 'Operated by ' + node.operator;
      const details = document.createElement('dl');
      for (const [label, value] of [['Hosted by', node.provider], ['Sponsor', node.sponsor || 'Not specified'], ['WHOIS endpoint', node.whois], ['Listing checked', node.verified_at?.slice(0, 10)]]) { const row = document.createElement('div'), dt = document.createElement('dt'), dd = document.createElement('dd'); dt.textContent = label; dd.textContent = value || 'Not supplied'; row.append(dt, dd); details.append(row); }
      const links = document.createElement('div'); links.className = 'node-links';
      const open = document.createElement('a'); open.className = 'button'; open.textContent = 'Open query tools →'; open.href = node.id === 'rirrd-uk-01' ? '/dashboard/' : node.https;
      const source = document.createElement('a'); source.textContent = 'Source coverage'; source.href = node.id === 'rirrd-uk-01' ? '/dashboard/#sources' : node.https;
      links.append(open, source); const note = document.createElement('p'); note.className = 'fine'; note.textContent = node.verification_scope || 'Reviewed operator listing.';
      card.append(top, title, intro, details, links, note); el('node-list').append(card);
    }
  }
  async function refresh() {
    if (document.hidden || loading) return; loading = true;
    try {
      const response = await fetch('/dashboard/live.json', {cache: 'no-store', signal: AbortSignal.timeout(10000)});
      if (!response.ok) throw new Error(); const live = await response.json(); if(live.service !== 'online') throw new Error();
      el('network-objects').textContent = number(live.total_objects);
      el('network-sources').textContent = live.sources.filter(row => row.state === 'loaded').length;
      const following = live.sources.filter(row => row.mirror?.state === 'following' && Date.now() / 1000 - row.mirror.checked_at_unix_seconds < 180).length;
      el('network-sync').textContent = following + ' incremental · remaining scheduled';
      el('network-sets').textContent = number(Object.values(live.class_counts || {}).reduce((n, row) => n + (row['as-set'] || 0), 0));
      const sources = live.sources.map(row=>({label:row.source,value:Object.values(live.class_counts?.[row.source] || {}).reduce((n,v)=>n+v,0)})).sort((a,b)=>b.value-a.value);
      const shown=sources.slice(0,8);if(sources.length>8)shown.push({label:'Other '+(sources.length-8)+' sources',value:sources.slice(8).reduce((n,row)=>n+row.value,0)});
      RIRRdCharts.bars(el('network-source-chart'),shown,'Current routing objects by source');
      try { const response=await fetch('/dashboard/history.json',{cache:'no-store'});if(!response.ok)throw new Error();const history=await response.json();RIRRdCharts.line(el('network-activity-chart'),history.samples,'http_qps','UK node HTTP queries per second',v=>v.toFixed(2)+'/s');}catch{RIRRdCharts.empty(el('network-activity-chart'),'Node activity history is temporarily unavailable.');}
      el('network-observed').textContent = 'UK node dataset · Observed ' + new Date(live.observed_at).toLocaleString() + ' · Per-source update modes are available in the dashboard.';
    } catch { el('network-observed').textContent = 'Live dataset counters are temporarily unavailable. Directory listings remain available.'; }
    finally { loading = false; }
  }
  el('site-query-form').addEventListener('submit', event=>{event.preventDefault();const parameters=new URLSearchParams({task:el('site-query-kind').value,value:el('site-query-value').value.trim()});location.href='/dashboard/?'+parameters+'#query';});
  el('region').addEventListener('change', render);
  document.querySelectorAll('[data-task]').forEach(link => { link.href = '/dashboard/?task=' + encodeURIComponent(link.dataset.task) + '#query'; });
  el('node-form').addEventListener('submit', async event => {
    event.preventDefault(); if (!el('node-form').reportValidity()) return;
    const values = Object.fromEntries(new FormData(el('node-form'))); values.whois_port = Number(values.whois_port); values.authorised = el('authorised').checked;
    el('node-submit').disabled = true; el('submission-status').textContent = 'Submitting for review…';
    try {
      const response = await fetch('/community/node-submissions', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(values), signal: AbortSignal.timeout(15000)});
      const data = await response.json(); if (!response.ok) throw new Error(data.error || 'Submission could not be stored.');
      el('submission-status').textContent = data.message + ' Reference: ' + data.id; el('node-form').reset();
    } catch (error) { el('submission-status').textContent = 'Not submitted: ' + (error.name === 'TimeoutError' ? 'The request timed out. Please retry.' : error.message); }
    finally { el('node-submit').disabled = false; }
  });
  fetch('/nodes.json', {cache: 'no-cache'}).then(response => { if (!response.ok) throw new Error(); return response.json(); }).then(data => { directory = data.nodes.filter(node => node.status === 'verified'); el('network-nodes').textContent = directory.length; render(); }).catch(() => { el('node-list').textContent = 'The node directory is temporarily unavailable.'; });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
  refresh(); setInterval(refresh, 60000);
})();
