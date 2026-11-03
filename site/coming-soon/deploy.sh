#!/bin/sh
# Run as root on a dedicated Debian host with the distribution caddy package.
set -eu
cd "$(dirname "$0")"
# Validate optional feature bundles before touching the deployed tree.
if [ -d public/dashboard ]; then
    for file in index.html style.css app.js meta.json sample-results.json network.json; do
        test -f "public/dashboard/$file"
    done
    python3 -c 'import json,re; m=json.load(open("public/dashboard/meta.json")); assert (m["mode"] == "preview" or (m["mode"] == "live" and m.get("public_service") is True)) and re.fullmatch("[0-9a-f]{40}",m["source_revision"])'
fi
caddy validate --config "$PWD/Caddyfile" --adapter caddyfile
backup_dir="/var/backups/rirrd-site/$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 700 "$backup_dir"
cp -a /etc/caddy/Caddyfile "$backup_dir/"
if [ -d /var/www/rirrd-coming-soon ]; then
    cp -a /var/www/rirrd-coming-soon "$backup_dir/"
fi
install -d -m 755 /var/www/rirrd-coming-soon
install -m 644 public/index.html public/style.css /var/www/rirrd-coming-soon/
if [ -f public/comparison.json ]; then
    install -m 644 public/comparison.json /var/www/rirrd-coming-soon/
fi
# Exact allowlist: no repository/configuration/runtime files enter the public root.
for file in node.css node.js project/index.html small-host.json small-host-cold.json license.txt dashboard/index.html dashboard/style.css dashboard/app.js dashboard/meta.json dashboard/sample-results.json dashboard/network.json guides/api/index.html guides/api/style.css guides/routers/index.html guides/routers/style.css guides/routers/bird.conf guides/routers/frr.conf; do
    if [ -f "public/$file" ]; then
        install -d -m 755 "/var/www/rirrd-coming-soon/$(dirname "$file")"
        install -m 644 "public/$file" "/var/www/rirrd-coming-soon/$file"
    fi
done
install -m 644 Caddyfile /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl enable caddy
systemctl reload-or-restart caddy
