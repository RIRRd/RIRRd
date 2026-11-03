#!/bin/sh
# Run as root on a dedicated Debian host with the distribution caddy package.
set -eu
cd "$(dirname "$0")"
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
install -m 644 Caddyfile /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl enable caddy
systemctl reload-or-restart caddy
