'use strict';
(() => {
  const el = id => document.getElementById(id);
  const text = (id, value) => { el(id).textContent = value; };
  const count = n => Number(n || 0).toLocaleString();
  const bytes = n => Number.isFinite(n) ? (n / 1048576).toFixed(1) + ' MiB' : 'Not reported';
  const pages = ['overview', 'query', 'sources', 'api', 'connect', 'system'];
  let metadata, latest, loading = false, currentResult = '', command = '', cursor = null, queryController;
  let historyData, historyFetched = 0;
  const publicMode = () => metadata?.public_service === true;
  const selectedPage = () => pages.includes(location.hash.slice(1)) ? location.hash.slice(1) : 'overview';
  const sampleASN = () => metadata?.sources?.includes('EXAMPLE') ? 'AS64496' : 'AS3333';
  const sampleSet = () => metadata?.sources?.includes('EXAMPLE') ? 'AS64496:AS-DOCS' : 'AS-RIPENCC';
  const sourceDefault = () => metadata?.sources?.includes('RIPE') ? 'RIPE' : metadata?.sources?.[0] || '';

  async function json(url, options = {}) {
    const response = await fetch(url, {cache: 'no-store', signal: AbortSignal.timeout(15000), ...options});
    if (!response.ok) throw new Error(response.status === 429 ? 'Too many requests. Wait a moment and retry.' : 'HTTP ' + response.status);
    const value = await response.json();
    if (value.errors?.length) throw new Error(value.errors.map(error => error.message).join('; '));
    return value;
  }
  function ago(value) {
    const seconds = Math.max(0, (Date.now() - new Date(value).getTime()) / 1000);
    if (!Number.isFinite(seconds)) return 'Unreported';
    return seconds < 60 ? 'Just now' : seconds < 3600 ? Math.floor(seconds / 60) + 'm ago' : seconds < 86400 ? Math.floor(seconds / 3600) + 'h ago' : Math.floor(seconds / 86400) + 'd ago';
  }
  function page() {
    let key = location.hash.slice(1) || 'overview';
    if (key === 'network' && publicMode()) { location.href = '/#nodes'; return; }
    key = selectedPage();
    pages.forEach(name => { el('page-' + name).hidden = key !== name; });
    document.querySelectorAll('[data-page]').forEach(link => {
      if (link.dataset.page === key) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current');
    });
    const title = document.querySelector('[data-page="' + key + '"]').textContent;
    text('breadcrumb', title); document.title = 'RIRRd · ' + title;
    if (metadata) refresh();
    window.scrollTo({top: 0});
  }
  function badge(label, warning = false) {
    const item = document.createElement('span'); item.className = 'badge' + (warning ? ' warning' : ''); item.textContent = label; return item;
  }
  function health(row) {
    if (row.state !== 'loaded') return ['Unavailable', true];
    const mirror = row.mirror;
    if (mirror) {
      const stale = Date.now() / 1000 - mirror.checked_at_unix_seconds > Math.max(180, mirror.interval_seconds * 3);
      return stale ? ['Check overdue', true] : mirror.state === 'following' ? ['Incremental', false] : ['Retrying mirror', true];
    }
    if (!publicMode()) return ['Local dataset', false];
    return [row.imported_at && Date.now() - Date.parse(row.imported_at) > 48 * 3600000 ? 'Snapshot overdue' : 'Scheduled snapshot', row.imported_at ? Date.now() - Date.parse(row.imported_at) > 48 * 3600000 : false];
  }
  function sourceRows() {
    if (latest?.sources && Array.isArray(latest.sources)) return latest.sources;
    return (metadata?.sources || []).map(source => ({source, state: 'loaded', mirror: latest?.sources?.[source], classes: latest?.class_counts?.[source]}));
  }
  function classes(row) { return latest?.class_counts?.[row.source] || row.classes || {}; }
  function fillSelect(id, names, all = false) {
    const target = el(id), selected = target.value, old = [...target.options].map(o => o.value);
    const values = all ? ['', ...names] : names;
    if (JSON.stringify(old) === JSON.stringify(values)) return;
    target.replaceChildren(); if (all) target.add(new Option('All configured sources', ''));
    names.forEach(name => target.add(new Option(name, name)));
    if (values.includes(selected)) target.value = selected;
  }
  function renderSources() {
    const rows = sourceRows(), filter = el('source-search').value.toUpperCase();
    el('source-rows').replaceChildren(); el('overview-sources').replaceChildren();
    let incremental = 0;
    for (const row of rows) {
      const [mode, warning] = health(row); if (mode === 'Incremental') incremental++;
      const button = document.createElement('button'); button.type = 'button'; button.textContent = row.source; button.title = mode;
      button.addEventListener('click', () => { el('source-detail').value = row.source; renderDetail(); location.hash = 'sources'; });
      el('overview-sources').append(button);
      if (!row.source.includes(filter)) continue;
      const counts = classes(row), current = Object.values(counts).reduce((a, b) => a + b, 0);
      const tr = document.createElement('tr'), th = document.createElement('th'), choose = button.cloneNode(true); th.scope = 'row';
      choose.addEventListener('click', () => { el('source-detail').value = row.source; renderDetail(); }); th.append(choose); tr.append(th);
      for (const value of [Object.keys(counts).length ? count(current) : row.objects == null ? '—' : count(row.objects), count(counts.route), count(counts.route6), count(counts['as-set'])]) { const td = document.createElement('td'); td.textContent = value; tr.append(td); }
      const state = document.createElement('td'); state.append(badge(mode, warning)); tr.append(state);
      const check = document.createElement('td'), when = row.mirror?.checked_at_unix_seconds ? new Date(row.mirror.checked_at_unix_seconds * 1000).toISOString() : row.imported_at;
      check.textContent = when ? ago(when) : 'Unreported'; check.title = when || ''; tr.append(check); el('source-rows').append(tr);
    }
    const names = rows.map(row => row.source);
    fillSelect('source-detail', names); fillSelect('browse-source', names);
    text('source-count', rows.length); text('source-summary', incremental + ' incremental · ' + (rows.length - incremental) + ' snapshot / other');
    text('freshness-summary', incremental + ' incremental feeds. ' + (rows.length - incremental) + ' sources use scheduled snapshots or have a different update state.');
    text('sources-state', rows.length + ' configured sources'); renderDetail();
  }
  function renderDetail() {
    const row = sourceRows().find(row => row.source === el('source-detail').value); if (!row) return;
    const out = el('source-detail-content'); const opened=new Set(out.dataset.source===row.source?[...out.querySelectorAll('details[open]')].map(item=>item.dataset.receipt):[]);out.dataset.source=row.source;out.replaceChildren();
    const summary = document.createElement('p'); summary.textContent = row.source + ' · ' + health(row)[0] + (row.provider ? ' · ' + row.provider : ''); out.append(summary);
    const grid = document.createElement('div'); grid.className = 'class-grid';
    for (const [name, total] of Object.entries(classes(row)).sort((a, b) => b[1] - a[1])) { const box = document.createElement('div'), label = document.createElement('span'), number = document.createElement('strong'); label.textContent = name; number.textContent = count(total); box.append(label, number); grid.append(box); }
    out.append(grid);
    const note = document.createElement('p'); note.className = 'fine'; note.textContent = row.mirror ? 'Committed mirror serial: ' + (row.mirror.serial ?? 'Not reported') + '. Last successful check: ' + (row.mirror.last_success_unix_seconds ? new Date(row.mirror.last_success_unix_seconds * 1000).toISOString() : 'Unreported') : 'Snapshot imported: ' + (row.imported_at || 'Unreported') + '. A snapshot is not a continuous feed.'; out.append(note);
    for (const file of row.downloads || []) {
      const details = document.createElement('details'), title = document.createElement('summary'), info = document.createElement('p'), hash = document.createElement('pre');
      details.dataset.receipt=file.url;details.open=opened.has(file.url);
      title.textContent = 'Publication receipt · ' + (file.url.split('/').pop() || row.source);
      info.className = 'fine'; info.textContent = file.url + ' · fetched ' + file.fetched_at + ' · ' + (file.transport_authenticated ? 'HTTPS' : 'Unauthenticated FTP') + (file.mirror_serial != null ? ' · peer serial ' + file.mirror_serial + ' · peer updated ' + file.mirror_last_update : '');
      hash.textContent = 'SHA-256: ' + file.sha256; details.append(title, info, hash); out.append(details);
    }
  }
  function renderHistory() {
    if (!historyData) return;
    const cutoff = Date.now() / 1000 - Number(el('history-range').value);
    const samples = historyData.samples.filter(p => p.timestamp >= cutoff);
    const charts = window.RIRRdCharts;
    const selected=selectedPage();
    if(selected==='overview'){
    charts.line(el('query-chart'), samples, 'http_qps', 'Public HTTP queries per second', v => v.toFixed(2) + '/s');
    charts.line(el('latency-chart'), samples, 'http_p95_ms', 'Recent HTTP p95 response time', v => v.toFixed(1) + ' ms');
    }
    if(selected==='system'){
    charts.line(el('memory-chart'), samples, 'rss_bytes', 'Registry process resident memory', v => (v / 1048576).toFixed(0) + ' MiB');
    charts.line(el('cpu-chart'), samples, 'cpu_percent', 'Registry CPU as a percentage of one core', v => v.toFixed(1) + '%');
    }
    text('history-note', historyData.scope + ' Samples: ' + samples.length + '. Collection began ' + (historyData.samples[0] ? new Date(historyData.samples[0].timestamp * 1000).toLocaleString() : 'just now') + '.');
  }
  async function refreshHistory(force) {
    if (!publicMode()) return;
    el('overview-history').hidden = false; el('resource-history').hidden = false;
    try {
      if (force || Date.now() - historyFetched > 60000) {
        historyData = await json('history.json'); historyFetched = Date.now();
      }
      renderHistory();
    } catch { text('history-note', 'History is temporarily unavailable. No values have been generated to fill the gap.'); }
  }
  async function refresh(force = false) {
    if (!metadata || loading || document.hidden) return;
    const selected = selectedPage();
    if (!force && !['overview', 'sources', 'system'].includes(selected)) return;
    loading = true;
    try {
      latest = await json(publicMode() ? 'live.json' : 'mirrors.json');
      if (publicMode() && latest.service !== 'online') throw new Error('Node service is unavailable');
      text('mode-badge', publicMode() ? 'Live · Beta' : 'Private instance');
      const totals = {};
      for (const group of Object.values(latest.class_counts || {})) for (const [name, value] of Object.entries(group)) totals[name] = (totals[name] || 0) + value;
      text('live-objects', count(latest.total_objects)); text('count-route', count(totals.route)); text('count-route6', count(totals.route6)); text('count-as-set', count(totals['as-set']));
      text('live-observed', latest.observed_at ? 'Observed ' + new Date(latest.observed_at).toLocaleString() + '. Counts include committed incremental edits.' : 'Committed database counts.');
      renderSources();
      RIRRdCharts.bars(el('source-chart'), sourceRows().map(row => ({label:row.source,value:Object.values(classes(row)).reduce((n,v)=>n+v,0)})).sort((a,b)=>b.value-a.value), 'Committed routing objects by source');
      if (['overview','system'].includes(selected) || force) await refreshHistory(force);
      el('resource-metrics').replaceChildren();
      for (const [label, value, detail] of [['Process memory', bytes(latest.process_rss_bytes), 'Resident memory (RSS)'], ['Service memory', bytes(latest.cgroup_memory_bytes), 'Includes charged filesystem cache'], ['CPU, one-core scale', latest.cpu_percent_one_core == null ? 'Not reported' : latest.cpu_percent_one_core.toFixed(2) + '%', 'Sampled, not lifetime CPU'], ['Database file', latest.database_bytes == null ? 'Not reported' : (latest.database_bytes / 1073741824).toFixed(2) + ' GiB', 'Current database generation']]) {
        const card = document.createElement('article'), name = document.createElement('span'), number = document.createElement('strong'), note = document.createElement('small'); name.textContent = label; number.textContent = value; note.textContent = detail; card.append(name, number, note); el('resource-metrics').append(card);
      }
      text('resource-note', publicMode() ? 'Only this node’s service is measured here. Other VM software and reclaimable OS cache are separate.' : 'Host resource telemetry is not exposed by this local console. Use your operating-system service tools for host measurements.');
    } catch (error) { if (publicMode()) text('mode-badge', 'Unavailable'); text('live-observed', 'Status unavailable: ' + error.message + '. Previously displayed figures may be stale.'); }
    finally { loading = false; }
  }
  function queryText() {
    const value = el('query-value').value.trim(), kind = el('query-kind').value, family = el('query-family').value;
    if (!value || value.length > 256 || !/^[a-z0-9][a-z0-9:._/ -]*$/i.test(value)) throw new Error('Enter an ASN, AS-set, IP address, prefix or object key.');
    if (kind === 'origin') { if (!/^AS\d+$/i.test(value) || Number(value.slice(2)) > 4294967295) throw new Error('Enter an ASN such as ' + sampleASN() + '.'); return (family === '6' ? '!6' : '!g') + value; }
    if (kind === 'recursive' || kind === 'members') return '!i' + value + (kind === 'recursive' ? ',1' : '');
    if (kind === 'set') return (family === '6' ? '!a6' : '!a4') + value;
    const isIP = /^(?:\d{1,3}\.){3}\d{1,3}(?:\/\d{1,2})?$/.test(value) || (value.includes(':') && /^[a-f0-9:]+(?:\/\d{1,3})?$/i.test(value));
    if (!isIP) { if (kind !== 'object') throw new Error('Enter an IP address or prefix.'); return value; }
    const prefix = value.includes('/') ? value : value + (value.includes(':') ? '/128' : '/32');
    return (kind === 'more' ? '-M ' : kind === 'covering' || !value.includes('/') ? '-L ' : '-x ') + prefix;
  }
  function requestPath(q) { const p = new URLSearchParams({q}); if (el('query-source').value) p.set('sources', el('query-source').value); return '/v1/whois/?' + p; }
  function updateCommand() {
    const kind=el('query-kind').value,family=el('query-family').value;
    document.querySelectorAll('[data-task]').forEach(button=>{
      const active=button.dataset.task===(kind==='origin'?'origin'+family:kind==='set'?'set'+family:kind);
      button.setAttribute('aria-pressed',String(active));
    });

    el('query-family').disabled = !['origin', 'set'].includes(el('query-kind').value);
    try { command = "curl --fail '" + location.origin + requestPath(queryText()) + "'"; } catch { command = 'Choose a query above.'; }
    text('query-command', command);
  }
  async function query(event) {
    event.preventDefault(); queryController?.abort(); const controller = new AbortController(); queryController = controller;
    const timeout = setTimeout(() => controller.abort(), 15000); el('query-submit').disabled = true; el('copy-result').disabled = true; text('query-status', 'Querying…'); text('result-count', '');
    try {
      const q = queryText(), kind = el('query-kind').value, source = el('query-source').value; updateCommand(); const response = await fetch(requestPath(q), {cache: 'no-store', signal: controller.signal});
      if (!response.ok) throw new Error(response.status === 429 ? 'Rate limit reached. Wait a moment and retry.' : 'Query failed (HTTP ' + response.status + '). Narrow the request or check the query type.');
      const raw = response.status === 204 ? '' : await response.text(); if (raw.length > 2097152) throw new Error('Result exceeds the display limit. Narrow the source or set.');
      if (queryController !== controller) return;
      const list = ['origin', 'set', 'members', 'recursive'].includes(kind);
      currentResult = list ? [...new Set(raw.trim().split(/\s+/).filter(Boolean))].join('\n') : raw.trim();
      text('query-result', currentResult || 'No matching records.'); text('result-count', currentResult ? list ? currentResult.split('\n').length + ' items' : 'RPSL response' : '0 items');
      text('query-status', currentResult ? 'Returned by ' + (source || 'configured source order') + '. Ready to copy.' : 'No matching records in this scope.'); el('copy-result').disabled = !currentResult;
    } catch (error) { if (queryController !== controller) return; currentResult = ''; text('query-result', 'No usable response was returned.'); text('query-status', error.name === 'AbortError' ? 'The query timed out or was cancelled. Try a narrower query.' : error.message); }
    finally { clearTimeout(timeout); if (queryController === controller) el('query-submit').disabled = false; }
  }
  function task(name, run = false) {
    const origin = name.startsWith('origin'), set = name.startsWith('set');
    el('query-kind').value = origin ? 'origin' : set ? 'set' : name === 'recursive' ? 'recursive' : 'object';
    el('query-family').value = name.endsWith('6') ? '6' : '4';
    el('query-value').value = set || name === 'recursive' ? sampleSet() : sampleASN();
    el('query-source').value = ''; updateCommand(); location.hash = 'query';
    if (run) el('query-form').requestSubmit(); else el('query-value').focus();
  }
  async function copy(value, button) {
    try { await navigator.clipboard.writeText(value); const label = button.textContent; button.textContent = 'Copied'; setTimeout(() => { button.textContent = label; }, 1500); }
    catch { text('query-status', 'Select and copy the displayed text.'); }
  }
  function commandCard(title, note, value) {
    const card = document.createElement('article'), heading = document.createElement('h2'), info = document.createElement('p'), code = document.createElement('pre'), button = document.createElement('button');
    card.className = 'command-card'; heading.textContent = title; info.textContent = note; code.textContent = value; button.type = 'button'; button.className = 'secondary'; button.textContent = 'Copy command'; button.addEventListener('click', () => copy(value, button)); card.append(heading, info, code, button); return card;
  }
  function commands() {
    const rawHost = metadata.whois_listen || location.hostname + ':43';
    const port = rawHost?.match(/:(\d+)$/)?.[1] || '4343';
    const boundHost = rawHost?.replace(/:\d+$/, '').replace(/^\[|\]$/g, '');
    const localOnly = !publicMode() && (boundHost === '::1' || /^127\./.test(boundHost || '') || boundHost === 'localhost');
    const browserHost = location.hostname.replace(/^\[|\]$/g, '');
    let host = publicMode() && boundHost && !['::','0.0.0.0','::1','localhost'].includes(boundHost) && !/^127\./.test(boundHost) ? boundHost : localOnly ? boundHost : browserHost;
    const whois = 'whois -h ' + host + (port === '43' ? '' : ' -p ' + port);
    const asn = sampleASN(), set = sampleSet(), source = sourceDefault();
    el('command-examples').replaceChildren();
    for (const [title, note, q] of [['IPv4 prefixes by ASN', 'Every matching IPv4 route in the selected sources.', '!g' + asn], ['IPv6 prefixes by ASN', 'The IPv6 counterpart of the origin lookup.', '!6' + asn], ['All ASNs in an AS-set', 'Recursive membership, with nested sets expanded.', '!i' + set + ',1'], ['AS-set to IPv4 prefixes', 'Resolve member ASNs to originated IPv4 routes.', '!a4' + set], ['AS-set to IPv6 prefixes', 'Resolve member ASNs to originated IPv6 routes.', '!a6' + set], ['Direct AS-set members', 'Includes nested set names without recursive expansion.', '!i' + set]]) {
      const p = new URLSearchParams({q, sources: source}); const value = whois + " '" + q + "'\n\n# HTTP, with explicit source selection\ncurl --fail '" + location.origin + '/v1/whois/?' + p + "'";
      el('command-examples').append(commandCard(title, note, value));
    }
    const endpoint = host.includes(':') ? '[' + host + ']:' + port : host + ':' + port;
    text('connect-endpoint', publicMode() ? 'Public WHOIS endpoint: ' + endpoint : localOnly ? 'This instance binds WHOIS locally at ' + endpoint + '. Run these commands on this host, or use an authorised management tunnel.' : 'WHOIS endpoint for this host: ' + endpoint + '. Use an address reachable from your router-management host.');
    el('router-commands').replaceChildren();
    const reserved = asn === 'AS64496' ? ' -p' : '';
    for (const [title, switches, name] of [['BIRD · IPv4 prefix set', '-4 -b', 'IRR_IN4'], ['BIRD · IPv6 prefix set', '-6 -b', 'IRR_IN6'], ['FRR · IPv4 prefix list', '-4', 'IRR_IN4'], ['FRR · IPv6 prefix list', '-6', 'IRR_IN6']]) el('router-commands').append(commandCard(title, 'Generate and review a candidate; choose your real peer or AS-set.', 'bgpq4' + reserved + ' ' + switches + ' -h ' + endpoint + ' -S ' + source + ' -l ' + name + ' ' + set));
    if (publicMode()) el('router-guide').href = '/guides/routers/';
  }
  async function browse(next = false) {
    el('browse-submit').disabled = true;
    try {
      const p = new URLSearchParams({source: el('browse-source').value, class: el('browse-class').value, limit: '30'}); if (next && cursor) p.set('after', cursor);
      const data = await json('/v1/irr/objects?' + p); cursor = data.next_cursor; el('browse-next').disabled = !cursor; el('browse-results').replaceChildren();
      data.keys.forEach(key => { const button = document.createElement('button'); button.type = 'button'; button.textContent = key; button.addEventListener('click', () => { el('query-value').value = ['route', 'route6'].includes(data.object_class) ? key.split('AS')[0] : key; el('query-kind').value = 'object'; el('query-source').value = data.source; updateCommand(); location.hash = 'query'; el('query-form').requestSubmit(); }); el('browse-results').append(button); });
      text('browse-status', data.keys.length + ' keys from ' + data.source + '. Select a key to inspect it. Live data can change between pages.');
    } catch (error) { text('browse-status', error.message); } finally { el('browse-submit').disabled = false; }
  }
  async function workbench(event, introspect = false) {
    event.preventDefault(); const button = el(introspect ? 'load-schema' : 'graphql-submit'); button.disabled = true;
    try { const query = introspect ? '{ __schema { queryType { name fields { name args { name type { kind name ofType { kind name ofType { kind name } } } } type { kind name ofType { kind name } } } } } }' : el('graphql-query').value; const data = await json('/graphql/', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({query})}); text(introspect ? 'schema-result' : 'graphql-result', JSON.stringify(data, null, 2)); text('graphql-status', 'Request completed.'); }
    catch (error) { text('graphql-status', error.message); } finally { button.disabled = false; }
  }
  async function initialize() {
    try {
      metadata = await json('meta.json');
      if (publicMode() && location.hash === '#network') { location.replace('/#nodes'); return; }
      text('mode-badge', publicMode() ? 'Live · Beta' : 'Private instance'); text('brand-caption', publicMode() ? 'Public node' : 'Your instance');
      if (publicMode()) { document.querySelector('.brand>span:last-child').firstChild.nodeValue = 'irrd.tools'; document.querySelector('.mobile-brand').textContent = 'irrd.tools'; }
      text('node-name', metadata.node?.name || 'RIRRd instance');
      text('node-description', publicMode() ? [metadata.node?.location, metadata.node?.provider ? 'Hosted by ' + metadata.node.provider : '', 'Read-only routing tools'].filter(Boolean).join(' · ') : 'Your independently operated Internet Routing Registry.');
      text('instance-label', publicMode() ? 'Public read-only node' : 'Independent self-hosted instance'); el('community-link').hidden = !publicMode();
      text('mode-notice', publicMode() ? 'Public beta · Read-only queries. Source update times are listed individually.' : 'Self-hosted RIRRd console · Independent of the public community network. No community account or node listing is required.');
      fillSelect('query-source', metadata.http_sources || metadata.sources, true); commands(); el('query-value').value = sampleASN(); updateCommand(); el('query-submit').disabled = false;
      const taskName = new URLSearchParams(location.search).get('task');
      if (['origin4','origin6','recursive','set4','set6','object'].includes(taskName)) task(taskName);
      const presetValue = new URLSearchParams(location.search).get('value'); if (presetValue && presetValue.length <= 256) { el('query-value').value = presetValue; updateCommand(); }
      const runSearch = new URLSearchParams(location.search).get('run') === '1' && ['origin4','origin6','recursive','set4','set6','object'].includes(taskName);
      if (runSearch) {
        // Consume the explicit user action once; reloading a copied URL is a preset.
        const url = new URL(location.href); url.searchParams.delete('run'); history.replaceState(null, '', url);
        el('query-form').requestSubmit();
      }
      await refresh();
    } catch (error) { text('mode-notice', 'Unable to connect: ' + error.message); el('mode-notice').className = 'notice error'; }
  }
  el('history-range').addEventListener('change', renderHistory);
  window.addEventListener('hashchange', page);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
  document.querySelectorAll('[data-task]').forEach(button => button.addEventListener('click', () => task(button.dataset.task)));
  ['query-value', 'query-kind', 'query-source', 'query-family'].forEach(id => el(id).addEventListener(id === 'query-value' ? 'input' : 'change', updateCommand));
  el('query-form').addEventListener('submit', query); el('refresh').addEventListener('click', () => refresh(true));
  el('copy-result').addEventListener('click', event => copy(currentResult, event.currentTarget)); el('copy-command').addEventListener('click', event => copy(command, event.currentTarget));
  el('source-search').addEventListener('input', renderSources); el('source-detail').addEventListener('change', renderDetail);
  ['browse-source', 'browse-class'].forEach(id => el(id).addEventListener('change', () => { cursor = null; el('browse-next').disabled = true; el('browse-results').replaceChildren(); text('browse-status', 'Choose Browse to load this selection.'); }));
  el('browse-form').addEventListener('submit', event => { event.preventDefault(); cursor = null; browse(); }); el('browse-next').addEventListener('click', () => browse(true));
  el('graphql-form').addEventListener('submit', event => workbench(event)); el('load-schema').addEventListener('click', event => workbench(event, true));
  page(); initialize(); setInterval(() => refresh(), 30000);
})();
