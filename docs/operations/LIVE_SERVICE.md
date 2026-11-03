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

## Source coverage and update modes

The public catalog is [sources.json](../../tools/live-service/sources.json).
It enables nineteen sources: RIPE, RIPE-NONAUTH, ARIN, APNIC, AFRINIC, LACNIC,
RADB, ALTDB, BELL, REACH, BBOI, TC, CANARIE, JPIRR, NTTCOM, IDNIC, LEVEL3,
PANIX and NESTEGG. Only a verified generation is exposed as loaded. All loaded
sources, including ARIN, are available through TCP WHOIS, HTTP WHOIS, GraphQL
and bounded key browsing. Source data comes from published public endpoints;
no owner enrollment is required by this deployment.

Seven configured sources follow NRTMv3 every sixty seconds. Other sources use
daily routing snapshots. IDNIC and LEVEL3 use the public WOBCOM IRRd GraphQL
mirror: all RPKI, scope and route-preference states are requested explicitly,
and the peer's serial and update marker must remain stable across collection.
Their direct bootstrap endpoints failed from this VM, so the working public
secondary mirror is used. The status page records that provider and its observed
source serial. JPIRR uses Merit's public snapshot; that peer's NRTMv3 transfer
returned a protocol access-denied response, so its working public snapshot is
used without blocking the source. PANIX and NESTEGG use the published Merit files.

The dashboard reports current source/class counts from a transactionally
maintained index, separate from bootstrap receipts and timestamps. Only routing
classes are retained: aut-num, as-set, route-set, route, route6, filter-set,
peering-set, rtr-set and inet-rtr, where published. Person/role, maintainer,
key-cert, address-allocation and organisation classes are excluded from this
routing mirror profile. Their absence is not an access-permission blocker.

Source mismatch and credential-bearing routing records fail the candidate.
Unsafe control-character records are withheld whole and counted. Duplicate
canonical identities use the last record, matching the pinned upstream snapshot
upsert policy; collapsed duplicates are counted. Normal CLI imports retain
duplicate rejection. Malformed optional lookup values may remain in source text
without being indexed and are counted separately. No lossy text decoding is used.

Download receipts retain provider URLs, hashes, timestamps and transport notes.
Anonymous FTP transport is unauthenticated. GraphQL receipts distinguish the
received JSON hash from deterministic gzip serialization. Imported source data
retains its own provenance; the project's MIT license covers project code.

An explicit `--previous-catalog` preparation resume can add feeds and change
query exposure without reimporting completed sources. It verifies and archives
the original inputs, rejects changed completed snapshot URLs/encodings/classes,
and performs complete final database verification. A committed incremental
cursor cannot be discarded by this operation. Runtime upgrades can switch a
verified database generation and executable together using refresh `--activate
GENERATION --runtime BINARY`; rollback restores the prior pointers and executable
without copying a multi-gigabyte serving database during downtime.

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

## Continuous feeds and snapshot fallback

The catalog configures RADB, ALTDB, BELL, REACH and BBOI for Merit's NRTMv3
endpoint, polling every 60 seconds after a matching snapshot/serial bootstrap.
Publication serials are checked before/after snapshot download; a changed serial
rejects the candidate. Incremental catch-up and full integrity verification run
before activation. Only source rows with an observed successful poll say
“following”; absent or failed telemetry never means fresh data.

Only the catalog's routing classes are retained from the incremental stream.
Persistent admission policy rejects configuration drift until an explicit
snapshot reset. Filtered mirrors keep no downstream journal; public mirroring
and submissions remain private. Ordered upstream omissions are valid NRTMv3,
but duplicate/backward/out-of-range operations and incomplete frames fail the
whole batch. Scheduled pulls stage at most 64 MiB with a 15-second deadline;
individual routing policies are capped at 4 MiB. These are operational bounds,
not claims of complete NRTM parity. NRTMv3 does not authenticate the publisher.

The dashboard shows last check, last success, serial, poll interval and overdue
or failed checks per source. The current aggregate object count includes
incremental edits; source/class counts explicitly describe the bootstrap.
The daily timer remains a full snapshot rebuild/recovery path. Other loaded
feeds stay daily snapshots until their own incremental bootstrap is verified.
See [the checkpoint](../compatibility/CONTINUOUS_MIRROR_CHECKPOINT.md).

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
headroom, bounded downloads with explicit transport policy, decompression/record budgets, source identity,
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

## Resume a failed private generation

Each successful source import writes an atomic checkpoint containing its normalized
snapshot hash, count, publication metadata and preparation-program hash. A failed
source leaves the live generation untouched. To resume, use the generation name
shown in the private refresh status/journal:

```sh
python3 /usr/local/lib/rirrd-live/refresh.py --resume GENERATION_NAME
```

Resume checks the exact catalog and recorded build fingerprint, probes the candidate
count and verifies completed snapshot hashes; it retries only unfinished feeds. It never modifies the
serving generation and refuses changed catalog/binary inputs. The final complete
integrity/count check and activation health checks still run. Runtime files and
checkpoint payloads remain outside Git. Unit/integration checks simulate failure
and resume using the normal Rust importer; the expanded live build also exercised
this path after TC's unsafe member records were detected.


## Activate a completed candidate with a verified runtime

A private prepared generation can be activated without redownloading it:

```sh
python3 /usr/local/lib/rirrd-live/refresh.py --activate GENERATION_NAME
```

The controller rejects a changed catalog, partial source set, inconsistent build
fingerprint or runtime integrity/count failure before switching. This path permits
an independently reviewed compatible runtime upgrade: the original build binary
fingerprint remains in the source report, and activation records the actual runtime
fingerprint separately. It validates the installed runtime's configuration and
complete database before the normal source-loss guard, switch, service health check
and automatic rollback. `--resume` still requires identical build inputs. Both
options reject the serving generation and take the same refresh lock.

Private staging may use a larger memory budget to keep verification's file cache
resident. This is separate from the public daemon's 2-GiB ceiling. RSS, charged
cgroup memory and whole-host RAM are different scopes; source import can populate
filesystem cache that is not charged to the query service.


For an explicitly reviewed compatible runtime change during private resume, pass
`--previous-binary /private/original-build` to `prepare.py --resume`. The original
hash must match the saved build inputs; the source catalog and normalized inputs
must still match. The original import fingerprint is preserved separately from
the new final-verification runtime. The complete post-catch-up verification is
mandatory; `database count` explicitly does not establish integrity. A prepared
activation can reuse that verification only if both the runtime fingerprint and
complete database and configuration fingerprints still match. If the runtime differs, it must run
a full integrity/count verification before switching. Daily bootstrap downloads
use a 60-second network-only CLI deadline; local indexing starts after staging.
