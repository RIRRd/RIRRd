#!/bin/sh
set -eu
cd "$(dirname "$0")"
test -f public/build.json
test -f public/dashboard/app.js
caddy validate --config "$PWD/Caddyfile" --adapter caddyfile
backup="/var/backups/rirrd-community/$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 700 "$backup"
cp -a /etc/caddy/Caddyfile "$backup/"
if [ -d /var/www/rirrd-community ]; then cp -a /var/www/rirrd-community "$backup/"; fi
if [ -f /usr/local/lib/rirrd-community/service.py ]; then cp -a /usr/local/lib/rirrd-community/service.py "$backup/"; fi
if ! getent passwd rirrd-community >/dev/null; then useradd --system --home-dir /var/lib/rirrd-community --shell /usr/sbin/nologin rirrd-community; fi
install -d -m 755 /usr/local/lib/rirrd-community
install -m 644 service.py /usr/local/lib/rirrd-community/service.py
install -m 644 rirrd-community.service /etc/systemd/system/rirrd-community.service
systemctl daemon-reload
systemctl enable --now rirrd-community.service
systemctl restart rirrd-community.service
curl --fail --silent --max-time 5 --retry 5 --retry-connrefused --retry-delay 1 http://127.0.0.1:9282/health >/dev/null
# Publish only manifest-approved files as one complete static release.
python3 - <<'PYDEPLOY'
from pathlib import Path
import datetime,hashlib,json,os,shutil
source=Path('public');manifest=json.loads((source/'build.json').read_text())
allowed={'index.html','community.css','community.js','nodes.json','license.txt','dashboard/index.html','dashboard/style.css','dashboard/app.js','guides/api/index.html','guides/routers/index.html','self-host/index.html','project/index.html','comparison.json'}
if not set(manifest['files']).issubset(allowed):raise ValueError('Unexpected public asset')
required=allowed-{'project/index.html','comparison.json'}
if not required.issubset(manifest['files']):raise ValueError('Incomplete public asset bundle')
release=Path('/var/www/rirrd-community-releases')/datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
release.mkdir(parents=True,mode=0o755)
for name,digest in manifest['files'].items():
    file=source/name
    if file.is_symlink() or hashlib.sha256(file.read_bytes()).hexdigest()!=digest:raise ValueError('Public asset fingerprint mismatch')
    destination=release/name;destination.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(file,destination);destination.chmod(0o644)
shutil.copyfile(source/'build.json',release/'build.json');(release/'build.json').chmod(0o644)
root=Path('/var/www/rirrd-community');pending=Path('/var/www/.rirrd-community-next')
if root.exists() and not root.is_symlink():raise ValueError('Expected a release symlink, found a directory; preserve it before switching')
pending.unlink(missing_ok=True);pending.symlink_to(release);os.replace(pending,root)
PYDEPLOY
install -m 644 Caddyfile /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl reload caddy
