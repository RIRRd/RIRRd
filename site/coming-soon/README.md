# Coming-soon website

Buildless static page, separate from all registry services. No remote assets or scripts.
On a dedicated Debian host install `caddy` from the distribution, then run
`sudo ./site/coming-soon/deploy.sh`. The script validates configuration, backs up
existing configuration and content under `/var/backups/rirrd-site/`, installs public
files under `/var/www/rirrd-coming-soon/`, and reloads the distribution `caddy.service`.

The Caddyfile serves only rirrd.co.uk. Caddy obtains and automatically renews public
TLS certificates and redirects HTTP to HTTPS. Keep `/var/lib/caddy/` persistent and
private. The admin API remains on loopback. Only TCP 80 and 443 are required; HTTP/3
is disabled. SSH must remain allowed. No registry port should be opened publicly.

Prerequisites: apex A and AAAA must reach this host. Validate IPv6 independently.
No www redirect is configured because that name has no DNS record. Check both
`curl -4 -I http://rirrd.co.uk` and `curl -6 -I http://rirrd.co.uk`, and HTTPS with
certificate validation. External verification must supplement local requests.

Rollback: restore Caddyfile and content from the timestamped backup, run
`caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile`, then reload Caddy.
Never copy certificate state, private keys, credentials or registry runtime data into Git.
