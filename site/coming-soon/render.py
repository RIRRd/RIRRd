#!/usr/bin/env python3
"""Render a buildless public comparison from a completed, validated scale run."""
import argparse
import datetime
import html
import json
import math
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('results', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parent
raw = json.loads(args.results.read_text())
assert raw['completed_at'] and raw['fixture']['objects'] == 100000
assert len(raw['imports']) == 3
assert raw['profiles'] and all(not p['errors'] and p['successful'] == p['attempted'] for p in raw['profiles'])
for imported in raw['imports']:
    assert all(imported[s]['accepted_objects'] == 100000 and imported[s]['exit_code'] == 0 for s in ('candidate', 'reference'))

def profiles(system, mode, connections='Reused', phase='workload'):
    return [p for p in raw['profiles'] if p['system'] == system and p['mode'].lower() == mode and p['connections'] == connections and p['phase'] == phase]

def median(items):
    return statistics.median(items)

def qps(value):
    return f'{value / 1000:,.1f}k' if value >= 10000 else f'{value:,.0f}'

def rate(system, mode):
    rows = profiles(system, mode)
    assert len(rows) == 3
    values = [p['queries_per_second'] for p in rows]
    display = (f'{min(values)/1000:.1f}k–{max(values)/1000:.1f}k' if max(values) >= 10000 else f'{min(values):,.0f}–{max(values):,.0f}')
    p95_bound = math.ceil(max(p['p95_ms'] for p in rows)*100)/100
    return f'{display} q/s', f'≤{p95_bound:.2f} ms p95', median(values)

def memory(value):
    return f'{value / (1024 ** 2):,.1f} MiB'

def difference(candidate, reference, higher=False):
    if higher:
        return f'{candidate / reference:.1f}× throughput' if candidate >= reference else f'{reference / candidate:.1f}× lower throughput'
    return f'{reference / candidate:.1f}× faster' if candidate <= reference else f'{candidate / reference:.1f}× slower'

rows = []
def row(label, note, candidate, reference, gain, candidate_note='', reference_note=''):
    esc = html.escape
    rows.append(f'<tr><th scope="row">{esc(label)}<small>{esc(note)}</small></th><td class="candidate">{esc(candidate)}' + (f'<small>{esc(candidate_note)}</small>' if candidate_note else '') + f'</td><td>{esc(reference)}' + (f'<small>{esc(reference_note)}</small>' if reference_note else '') + f'</td><td class="gain">{esc(gain)}</td></tr>')

imports = {s: median([i[s]['seconds'] for i in raw['imports']]) for s in ('candidate','reference')}
row('Snapshot import', 'Median of three fresh databases', f'{imports["candidate"]:.2f} s', f'{imports["reference"]:.2f} s', difference(imports['candidate'], imports['reference']), f'{min(i["candidate"]["seconds"] for i in raw["imports"]):.2f}–{max(i["candidate"]["seconds"] for i in raw["imports"]):.2f} s across runs', f'{min(i["reference"]["seconds"] for i in raw["imports"]):.2f}–{max(i["reference"]["seconds"] for i in raw["imports"]):.2f} s across runs')
row('Import throughput', '100,000 objects / median import time', f'{100000 / imports["candidate"]:,.0f} objects/s', f'{100000 / imports["reference"]:,.0f} objects/s', difference(100000/imports['candidate'],100000/imports['reference'],True))
disk = raw['disk']
reduction = 100 * (1-disk['candidate_allocated_bytes']/disk['reference_allocated_bytes'])
row('Database on disk', 'Allocated bytes; PostgreSQL includes WAL', memory(disk['candidate_allocated_bytes']), memory(disk['reference_allocated_bytes']), f'{reduction:.1f}% smaller')
mem = raw['idle_pss']
mem_reduction = 100 * (1-mem['candidate']/mem['reference'])
row('Idle process memory', 'Median PSS; whole query stack', memory(mem['candidate']), memory(mem['reference']), f'{mem_reduction:.1f}% lower')
rates = {}
for mode, label in [('whois', 'WHOIS'), ('graphql', 'GraphQL')]:
    current, baseline = rate('candidate', mode), rate('reference', mode)
    rates[mode] = (current, baseline)
    row(label + ' throughput', 'Reused connections · three 20 s runs', current[0], baseline[0], difference(current[2],baseline[2],True), current[1],baseline[1])

highlight_values = [(f'{imports["reference"]/imports["candidate"]:.1f}×', 'Faster snapshot import', 'Median of three clean imports'), (f'{mem_reduction:.1f}%', 'Lower idle process memory', 'Whole-stack process PSS'), (f'{rates["whois"][0][2]/rates["whois"][1][2]:.1f}×', 'WHOIS throughput', 'Ratio of median measured rates')]
highlights = ''.join(f'<article class="highlight"><p class="highlight-label">{html.escape(label)}</p><strong>{html.escape(value)}</strong><p>{html.escape(note)}</p></article>' for value,label,note in highlight_values)
more = '<h4>New connection per request</h4><ul>'
for mode,label in [('whois','WHOIS'),('graphql','GraphQL')]:
    pair = {s: profiles(s, mode, 'New') for s in ('candidate','reference')}
    assert all(len(p) == 1 for p in pair.values())
    a,b=pair['candidate'][0],pair['reference'][0]
    more += f'<li>{label}, single 10-second run: RIRRd {qps(a["queries_per_second"])} q/s (p95 {a["p95_ms"]:.2f} ms); IRRd {qps(b["queries_per_second"])} q/s (p95 {b["p95_ms"]:.2f} ms).</li>'
more += '</ul><h4>Tail latency on reused connections</h4><ul>'
for mode, label in [('whois','WHOIS'),('graphql','GraphQL')]:
    pair = {s: profiles(s, mode) for s in ('candidate','reference')}
    a,b=pair['candidate'],pair['reference']
    more += f'<li>{label}, worst per-run p99: RIRRd {max(p["p99_ms"] for p in a):.2f} ms; IRRd {max(p["p99_ms"] for p in b):.2f} ms. Maximum observed request: {max(p["max_ms"] for p in a):.2f} ms and {max(p["max_ms"] for p in b):.2f} ms respectively.</li>'
more += '</ul><h4>Import CLI resources</h4>'
cli_rss = {s: median([i[s]['peak_cli_rss_bytes'] for i in raw['imports']]) for s in ('candidate','reference')}
more += f'<p>Median peak import CLI RSS: RIRRd {memory(cli_rss["candidate"])}; IRRd {memory(cli_rss["reference"])}. These component figures exclude separate PostgreSQL/Redis processes and are not whole-stack peak memory.</p>'
more += '<h4>GraphQL optimization check</h4>' 
optimization = [p for p in raw['profiles'] if p['phase'] == 'optimization']
old = [p['queries_per_second'] for p in optimization if p['variant'] == 'before']
new = [p['queries_per_second'] for p in optimization if p['variant'] == 'current']
assert len(old) == len(new) == 3
more += f'<p>Three alternating 10-second runs of the previous checkpoint binary and this checkpoint on the same database: median {qps(median(old))} → {qps(median(new))} q/s. This checks the common identity-only query after removing eager GraphQL text copies and attribute parsing. It does not measure all field selections. The run ranges overlap, so this short check does not establish a throughput improvement.</p>'
text = (root/'templates/index.html').read_text()
values = {'HIGHLIGHTS':highlights,'ROWS':''.join(rows),'DATE':datetime.datetime.fromisoformat(raw['completed_at'].replace('Z','+00:00')).strftime('%d %b %Y'),'REQUESTS':f'{sum(p["successful"] for p in raw["profiles"]):,}','MORE':more,'COMMIT':html.escape(raw['candidate_commit'])}
for key,value in values.items():
    text=text.replace('@@'+key+'@@',value)
assert '@@' not in text
(root/'public/index.html').write_text(text)
(root/'public/comparison.json').write_text(json.dumps(raw,indent=2)+'\n')
print('Rendered validated benchmark:', raw['completed_at'])
