The current public deployment serves the operational portal at `/`, historical
measured comparisons at `/project/`, the full dashboard at `/dashboard/`, and
commands at `/guides/api/` and `/guides/routers/`. Use the clean core checkout
with `sync_dashboard.py --live` to regenerate the live assets. Source coverage,
embedded RPKI status and deployment evidence are recorded in the
[UK node checkpoint](../../docs/compatibility/UK_NODE_PUBLIC_SOURCES_RPKI.md).

# Coming-soon website

Static project page and local dashboard assets. Only an explicit read-only query/status allowlist is proxied to the local gateway. No remote assets or third-party scripts.
On a dedicated Debian host install `caddy` from the distribution, then run
`sudo ./site/coming-soon/deploy.sh`. The script validates configuration, backs up
existing configuration and content under `/var/backups/rirrd-site/`, installs public
files under `/var/www/rirrd-coming-soon/`, and reloads the distribution `caddy.service`.

The Caddyfile serves only rirrd.co.uk. Caddy obtains and automatically renews public
TLS certificates and redirects HTTP to HTTPS. Keep `/var/lib/caddy/` persistent and
private. The admin API remains on loopback. Only TCP 80 and 443 are required; HTTP/3
is disabled. SSH must remain allowed. The user authorized the experimental public read-only registry: TCP 43 is open for WHOIS/bgpq4; submissions and development ports remain private.

Prerequisites: apex A and AAAA must reach this host. Validate IPv6 independently.
No www redirect is configured because that name has no DNS record. Check both
`curl -4 -I http://rirrd.co.uk` and `curl -6 -I http://rirrd.co.uk`, and HTTPS with
certificate validation. External verification must supplement local requests.

Rollback: restore Caddyfile and content from the timestamped backup, run
`caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile`, then reload Caddy.
Never copy certificate state, private keys, credentials or registry runtime data into Git.

## Updating the comparison

The published page is generated, but remains buildless at runtime. Keep the
layout in `templates/index.html`, styles in `public/style.css`, and regenerate:

```sh
python3 site/coming-soon/render.py /private/path/to/completed-scale-results.json
```

The renderer requires three fresh imports and all three main workload
repetitions, rejects error-bearing profiles, and checks object counts. It
produces `public/index.html` and `public/comparison.json`. Inspect the raw JSON
before committing: no private fixtures, passwords, host secrets or local paths
belong in a public result. This measurement uses only synthetic objects.

Headline factors use ratios of median run statistics. Query throughput ranges
include every main repetition; the displayed latency bound is the worst per-run
p95, not maximum latency. The page includes memory/disk accounting and the limits
of the synthetic workload. Never replace measurements with illustrative figures
or present this checkpoint as full parity or production qualification.

Deploy with `./site/coming-soon/deploy.sh` after inspecting the generated page.
Verify IPv4 and IPv6 HTTPS, HTTP redirects, and an external vantage where
available. Keep the repository private and all registry services isolated.

## Bundled dashboard preview and public guides

The live Rust console is served privately at `/ui/` by the registry's optional
loopback HTTP listener. The public `/dashboard/` uses the real local RIRRd query service and measured telemetry. The builder also supports an explicitly synthetic preview mode for local demos. The community directory
is empty until real operators, locations and sponsors are confirmed.

Install the distribution `python3-markdown` build tool, then sync from a clean
core checkout:

```sh
python3 site/coming-soon/sync_dashboard.py /path/to/core-checkout --live
python3 site/coming-soon/render.py /private/large-comparison.json --small-host /private/small-host.json --cold-start /private/cold-recheck.json
```

The source revision is embedded in preview metadata. `--allow-dirty` is for local
preview only; deployment requires a committed revision and either preview mode or explicit public live mode. The guide
is rendered from the maintained core Markdown and the router templates are copied
as synthetic examples. No runtime database, keys or host configuration is copied.

Caddy permits local scripts and same-origin JSON requests only under `/dashboard/`.
The landing page and guides retain their script-free policy. Deployment installs
an exact allowlist of public assets. All project code/assets here are MIT-licensed.
The AI disclosure is visible on the landing page, dashboard and guide; AI-assisted
contributions are welcome with review and testing required.

## Live query deployment

Install/verify the service and gateway using the core repository's
`docs/operations/LIVE_SERVICE.md` before deploying this Caddy configuration.
Only `/graphql/`, `/v1/whois/`, `/dashboard/meta.json` and `/dashboard/live.json`
reach loopback port 9281. The gateway handles its route/method allowlist and
rate/body/concurrency/response budgets; Rust HTTP remains private on port 9280.
Caddy overwrites the client-address header and backend Host. The remaining
paths serve only the exact public file allowlist. `/v1/submit/` stays 404.

Live counts/timestamps/resources come from verified runtime state, never the
synthetic comparison JSON. Current source coverage and pending feed actions
are displayed, including ARIN's TCP-only distribution restriction. Full parity
and production readiness remain incomplete. The repository remains private.
