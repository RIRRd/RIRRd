#!/bin/sh
# Deploy both separately built sites without stopping or copying registry data.
set -eu
cd "$(dirname "$0")/.."
test -f site/project/public/build.json
test -f site/community/public/build.json
caddy validate --config "$PWD/site/Caddyfile" --adapter caddyfile
backup="/var/backups/rirrd-web/$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 700 "$backup"
cp -a /etc/caddy/Caddyfile "$backup/"
for path in /etc/systemd/system/rirrd-community.service /usr/local/lib/rirrd-community/service.py /usr/local/lib/rirrd-live/gateway.py /usr/local/lib/rirrd-live/node.json; do
    if [ -f "$path" ]; then cp -a "$path" "$backup/"; fi
done
for path in /var/www/rirrd-project /var/www/irrd-tools; do
    if [ -e "$path" ]; then cp -a "$path" "$backup/"; fi
done
if ! getent passwd rirrd-community >/dev/null; then useradd --system --home-dir /var/lib/rirrd-community --shell /usr/sbin/nologin rirrd-community; fi
install -d -m 755 /usr/local/lib/rirrd-community
install -m 644 site/community/service.py /usr/local/lib/rirrd-community/service.py
install -m 644 site/community/rirrd-community.service /etc/systemd/system/rirrd-community.service
install -m 644 tools/live-service/gateway.py tools/live-service/node.json /usr/local/lib/rirrd-live/
install -d -m 755 /etc/systemd/system/rirrd-live-gateway.service.d
cat > /etc/systemd/system/rirrd-live-gateway.service.d/telemetry.conf <<'EOFUNIT'
[Service]
StateDirectory=rirrd-telemetry
StateDirectoryMode=0700
UMask=0077
ExecStart=
ExecStart=/usr/bin/python3 /usr/local/lib/rirrd-live/gateway.py --generation /var/lib/rirrd-live/current --history-file /var/lib/rirrd-telemetry/history.json
EOFUNIT
systemctl daemon-reload
systemctl enable --now rirrd-community.service
systemctl restart rirrd-community.service rirrd-live-gateway.service
curl --fail --silent --max-time 5 --retry 5 --retry-connrefused --retry-delay 1 http://127.0.0.1:9282/health >/dev/null
python3 - <<'PYDEPLOY'
from pathlib import Path
import datetime,hashlib,json,os,shutil
stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
project={'index.html','project.css','project.js','brand/rirrd.svg','charts.js','comparison.json','license.txt',*[f'docs/{p}/index.html' for p in ['getting-started','api','routers','rpki','beta']]}
community={'index.html','community.css','community.js','nodes.json','license.txt','brand/irrd-tools.svg',*[f'dashboard/{p}' for p in ['index.html','style.css','app.js','charts.js','mark.svg']],*[f'{p}/index.html' for p in ['guides/api','guides/routers','self-host','project']],'comparison.json'}
prepared=[]
for product,source,allowed in [('rirrd-project',Path('site/project/public'),project),('irrd-tools',Path('site/community/public'),community)]:
    manifest=json.loads((source/'build.json').read_text())
    if set(manifest['files'])!=allowed:raise ValueError('Unexpected or incomplete public asset manifest: '+product)
    release=Path('/var/www')/(product+'-releases')/stamp;release.mkdir(parents=True,mode=0o755)
    for name,digest in manifest['files'].items():
        file=source/name
        if file.is_symlink() or hashlib.sha256(file.read_bytes()).hexdigest()!=digest:raise ValueError('Asset fingerprint mismatch')
        dest=release/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(file,dest);dest.chmod(0o644)
    shutil.copyfile(source/'build.json',release/'build.json');(release/'build.json').chmod(0o644)
    root=Path('/var/www')/product
    if root.exists() and not root.is_symlink():raise ValueError('Preserve the existing non-symlink web root first')
    prepared.append((root,release))
for root,release in prepared:
    pending=root.with_name('.'+root.name+'-next');pending.unlink(missing_ok=True);pending.symlink_to(release);os.replace(pending,root)
PYDEPLOY
install -m 644 site/Caddyfile /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl reload caddy
