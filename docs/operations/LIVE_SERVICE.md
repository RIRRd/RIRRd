# Experimental public read-only service

The user authorized the public test registry on 3 November 2026. This supersedes
the earlier static-only deployment restriction for **read-only queries**. No public
authoritative submission, account administration, journal or mirroring endpoint
is enabled. Full IRRd parity and production qualification remain incomplete.

- Dashboard: https://rirrd.co.uk/dashboard/
- WHOIS/bgpq4: `rirrd.co.uk`, TCP port 43, IPv4 and IPv6.
- HTTP WHOIS: `GET /v1/whois/?q=AS3333`.
- GraphQL: `POST /graphql/`, JSON body, bounded read-only schema.
- Aggregate live telemetry: `GET /dashboard/live.json`.

The [router guide](ROUTER_INTEGRATION.md) shows public tests and private synthetic
evaluation, source selection, candidate validation and rollback. No configuration
change to a router is performed automatically by this project.

## Data scope and permissions

The reviewed source catalog is [sources.json](../../tools/live-service/sources.json).
The public status API shows only successfully imported sources as loaded, with
per-class counts, source URLs/hashes, upstream Last-Modified and local import times.
Pending feeds are visibly separate. No fallback forwards queries to another IRR;
query results come from this RIRRd database.

Initial enabled feeds are RIPE, RIPE-NONAUTH, ARIN, AFRINIC and LACNIC. Only routing
classes are retained: aut-num, as-set, route-set, route, route6, filter-set,
peering-set, rtr-set and inet-rtr (where published). Person/role, maintainer,
key-cert, address-allocation and organisation classes are not imported. Class
exclusions are counted; selected malformed records fail the entire candidate. Published duplicate canonical
identities use the last record, matching the pinned upstream snapshot upsert policy;
collapsed duplicates are counted. Normal CLI imports retain duplicate rejection.
Malformed optional lookup values can remain in source text without indexing;
the importer reports their count separately. UTF-8 RIPE exports and explicitly
declared source encodings are used; no lossy decode is allowed.

Source data is **not MIT-licensed by this project**. Its original terms continue
to apply. This deployment is for Internet routing operations/testing, with no
marketing use or downloadable database export. Permission references:

- [RIPE public daily snapshots](https://docs.db.ripe.net/FAQ) and
  [database terms](https://docs.db.ripe.net/HTML-Terms-And-Conditions).
- [ARIN IRR downloads](https://www.arin.net/resources/manage/irr/) and
  [NRPM 3.4.1](https://www.arin.net/participate/policy/nrpm/#3-4-routing-registry).
  ARIN permits routing-registry mirroring for Internet operations and restricts
  redistribution to WHOIS absent written permission. Consequently ARIN is
  available through TCP WHOIS only here: `http_sources` excludes it from **all**
  HTTP queries, including implicit defaults and explicit source selection.
- [AFRINIC's mirroring instructions](https://afrinic.net/ast/pdf/afrinic-irr-guide-20180927.pdf).
  Continuous NRTM needs IP whitelisting, which is not assumed from snapshot access.
- [LACNIC's open mirroring policy](https://www.lacnic.net/5009/2/lacnic/update-on-lacnics-irr:-near-real-time-mirroring-now-available).

The remaining irr.fyi-listed feeds are inventoried without pretending they are
loaded. APNIC requires its [signed bulk-access AUP](https://www.apnic.net/manage-ip/using-whois/bulk-access/).
NTT asks mirrors to [contact its registry administrator](https://www.gin.ntt.net/support-center/policies-procedures/routing-registry/).
JPIRR has a separate mirror policy/onboarding process. RADB's FTP listing is
reachable, but an approved transport/redistribution configuration is still pending.
ALTDB's HTTPS hostname fails certificate validation, LEVEL3 times out and IDNIC
fails TLS negotiation from this VM. BELL, REACH, TC, CANARIE and BBOI remain in
permission/transport review. Do not disable certificate checks or assume a mirror
has granted downstream redistribution of every third-party dataset.

## Isolation and limits

Distribution Caddy terminates TLS, retains automatic renewal and serves project
pages from `/var/www/rirrd-coming-soon`. Its narrow query allowlist proxies to a
loopback-only Python gateway on port 9281; all other registry routes remain 404.
The Rust HTTP service stays on loopback port 9280 and validates Host. The gateway
uses a literal loopback backend, never an arbitrary user-supplied URL.

The gateway permits only WHOIS GET and GraphQL POST. It limits request bodies to
16 KiB, responses to 2 MiB, concurrent worker threads to 16 and query rate to two
per second with a 20-request burst per IP (IPv6 /64). Caddy overwrites the client
address header from its direct connection. No visitor IP/query logs or third-party
analytics are enabled. Rate state is bounded to 4,096 in-memory client keys.

Rust WHOIS has 128 global connections, eight per IP and a ten-second I/O deadline.
Its NRTM ACL is empty and authoritative sources/HTTP updates are disabled. The
unprivileged daemon has only CAP_NET_BIND_SERVICE, a 64 MiB database cache, two
runtime workers, a 2 GiB cgroup ceiling and a three-CPU quota. These are operational
ceilings for the larger real dataset, not measured minimum-host requirements.
The gateway has a separate 128 MiB ceiling. Firewall input permits TCP 22/43/80/443,
loopback, established traffic and ICMP; development ports remain private.

## Measured telemetry

The gateway samples every five seconds; the visible dashboard refreshes every
15 seconds while visible. It reports daemon RSS, service cgroup memory/peak, CPU
percentage (100% = one core), daemon uptime, current established TCP WHOIS sockets, apparent database file size and
verified snapshot object counts. HTTP attempts/errors reset on gateway restart;
recent p95 uses the latest 512 gateway attempts, including proxy overhead. TCP
query counts are not measured and are explicitly excluded. Failed telemetry is
marked unavailable rather than replaced with example numbers. Displayed snapshot
times are not proof of upstream completeness or live RPKI/scope filtering.

## Build, refresh and rollback

Reusable files are in [tools/live-service](../../tools/live-service). Install the
verified release binary as `/usr/local/libexec/rirrd-live`, Python files/catalog
under `/usr/local/lib/rirrd-live`, and systemd units under `/etc/systemd/system`.
Use the existing unprivileged `rust-irrd` account. Keep `/var/lib/rirrd-live`
root-owned; only each generation's database file is writable by the daemon.

`prepare.py --binary <immutable-binary> --output <new-generation>` checks disk
headroom, bounded HTTPS downloads, decompression/record budgets, source identity,
credential-field exclusion, strict imports and complete database integrity. A
generation is not publishable unless every enabled source succeeds. Snapshot
objects have a 4 MiB limit; authoritative submission admission remains 64 KiB.

`rirrd-live-refresh.timer` runs daily after 03:30 UTC with up to 30 minutes jitter.
`refresh.py` takes a lock, builds while the old generation serves, rejects source
loss or a greater-than-20% count drop, switches the root-owned `current` symlink,
restarts Rust and checks its normal HTTP source-status entry point. Failed startup
restores and restarts the previous generation. Detailed errors stay in the private
journal; public status says that the last verified generation was retained. Only
the latest and preceding complete generations are retained after success.

```sh
systemctl status rirrd-live rirrd-live-gateway rirrd-live-refresh.timer
journalctl -u rirrd-live-refresh.service
systemctl start rirrd-live-refresh.service
```

For manual rollback, stop the daemon, point `current` to the retained previously
verified generation, start it, then check both TCP WHOIS and HTTP source status.
Preserve an offline backup before migrations. Site/config backups are separate
under `/var/backups/rirrd-site/`. Keep all snapshots, databases, host runtime and
credential material outside Git.
