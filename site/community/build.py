#!/usr/bin/env python3
"""Build the independent community website using the shared instance console."""
import argparse
import html
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import markdown

ROOT = Path(__file__).resolve().parent


def document(title, body):
    return f'''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{html.escape(title)} · irrd.tools</title><link rel="icon" href="/brand/irrd-tools.svg" type="image/svg+xml"><link rel="stylesheet" href="/community.css"><script src="/community.js" defer></script></head><body><a class="skip" href="#main">Skip to content</a><header class="shell masthead"><a class="brand" href="/"><img src="/brand/irrd-tools.svg" width="40" height="40" alt=""><strong>irrd.tools</strong></a><button class="menu-toggle secondary" type="button" aria-expanded="false" aria-controls="site-navigation">Menu</button><nav id="site-navigation" aria-label="Main"><a href="/#nodes">Public nodes</a><a href="/dashboard/#query">Query tools</a><a href="/guides/api/">API</a><a href="https://rirrd.co.uk/">RIRRd project ↗</a><a class="mobile-only" href="/#submit-node">Add your node</a></nav><a class="button secondary" href="/#submit-node">Add your node</a></header><main id="main" class="shell document">{body}</main><footer class="shell"><div>irrd.tools · <a href="https://rirrd.co.uk/">Powered by RIRRd</a></div><div><a href="https://github.com/RIRRd/RIRRd/issues">Report a beta issue</a></div><p>MIT-licensed code · AI-assisted development · Public beta</p></footer></body></html>'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, default=ROOT.parents[1])
    parser.add_argument('--output', type=Path, default=ROOT / 'public')
    parser.add_argument('--allow-dirty', action='store_true', help='Local preview only; commit before publishing')
    args = parser.parse_args()
    core = args.core.resolve()
    if subprocess.check_output(['git', '-C', str(core), 'status', '--porcelain']) and not args.allow_dirty:
        parser.error('Use a clean committed source checkout for publishing')
    revision = subprocess.check_output(['git', '-C', str(core), 'rev-parse', 'HEAD'], text=True).strip()
    out = args.output
    out.mkdir(parents=True, exist_ok=True)
    for source, target in [('index.html', 'index.html'), ('community.css', 'community.css'), ('community.js', 'community.js')]:
        shutil.copyfile(ROOT / 'assets' / source, out / target)
    shutil.copyfile(ROOT / 'nodes.json', out / 'nodes.json')
    shutil.copyfile(core / 'LICENSE', out / 'license.txt')
    dash = out / 'dashboard'
    dash.mkdir(exist_ok=True)
    for name in ['index.html', 'style.css', 'app.js', 'charts.js']:
        shutil.copyfile(core / 'web/dashboard' / name, dash / name)
    shutil.copyfile(core / 'site/brand/irrd-tools.svg', dash / 'mark.svg')
    (out / 'brand').mkdir(exist_ok=True)
    shutil.copyfile(core / 'site/brand/irrd-tools.svg', out / 'brand/irrd-tools.svg')
    # The live gateway supplies meta/live JSON. A static manifest records build provenance.
    (out / 'build.json').write_text(json.dumps({'source_revision': revision, 'product': 'irrd.tools community network', 'console_source': 'web/dashboard', 'submission_service': 'separate community service'}, indent=2) + '\n')
    for folder, title, path in [('guides/api','API and query examples',core/'docs/operations/API_COMMANDS.md'), ('guides/routers','Use the public node with BIRD or FRR',core/'docs/operations/PUBLIC_ROUTER_QUERIES.md'), ('self-host','Self-host RIRRd',ROOT/'SELF_HOST.md')]:
        destination = out / folder
        destination.mkdir(parents=True, exist_ok=True)
        body = markdown.markdown(path.read_text(), extensions=['fenced_code', 'tables'])
        (destination / 'index.html').write_text(document(title, body))
    # Keep measured historical results available, without treating them as live capacity.
    old = core / 'site/coming-soon/public'
    if (old / 'comparison.json').exists():
        shutil.copyfile(old / 'comparison.json', out / 'comparison.json')
        page = (old / 'project/index.html').read_text()
        start = page.index('<table class="metrics">')
        end = page.index('</table>', start) + len('</table>')
        table = page[start:end]
        project = out / 'project'
        project.mkdir(exist_ok=True)
        body = '<h1>Self-hosted software &amp; measured comparisons</h1><p>RIRRd software is independent of the community network. <a href="/self-host/">Read about self-hosting →</a></p><h2>Recorded IRR-only comparison</h2><p>100,000 synthetic objects on the recorded Linux host, against pinned official IRRd 4.5.4. These historical measurements use their recorded commits with RPKI disabled; they are not current capacity or minimum host requirements.</p><div class="table-scroll">' + table + '</div><p><a href="/comparison.json">Raw results and exact tested revisions</a></p><p>Live node counts, memory and source health are in the <a href="/dashboard/">public node console</a>. Full feature parity and production qualification remain incomplete.</p>'
        (project / 'index.html').write_text(document('Software and measured comparisons', body))
    files = ['index.html','community.css','community.js','nodes.json','license.txt',
             'dashboard/index.html','dashboard/style.css','dashboard/app.js','dashboard/charts.js','dashboard/mark.svg','brand/irrd-tools.svg',
             'guides/api/index.html','guides/routers/index.html','self-host/index.html']
    if (old / 'comparison.json').exists():
        files += ['project/index.html','comparison.json']
    (out / 'build.json').write_text(json.dumps({'source_revision':revision,
        'product':'irrd.tools community network','files':{name:hashlib.sha256((out/name).read_bytes()).hexdigest() for name in files}},indent=2)+'\n')
    print('Built community site from', revision, 'at', out)


if __name__ == '__main__':
    main()
