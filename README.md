# RIRRd

**Beta 0.1.0-beta.2** · [MIT license](LICENSE) · [Project website](https://rirrd.net/)
· [Community tools](https://irrd.tools/) · [Beta status](docs/BETA_STATUS.md)

A Rust Internet Routing Registry daemon with embedded storage, bounded WHOIS,
HTTP and GraphQL queries and a self-hosted console. PostgreSQL and Redis are not runtime dependencies. The project aims
to make running an IRR service practical on smaller Linux hosts.

**This is beta software. Full IRRd parity and production qualification remain
incomplete.** The baseline is official IRRd v4.5.4 at
`8e8080dab744a348b7d0e6efcb53f403301dd075`. Check the
[compatibility matrix](docs/compatibility/IRRD_FEATURE_MATRIX.md),
[known gaps](docs/BETA_STATUS.md) and [development status](docs/STATUS.md).
The MIT-licensed source and public issue tracker are hosted at
[RIRRd/RIRRd](https://github.com/RIRRd/RIRRd).
Release artifacts are published only after the release checks pass.

## Development

Active changes are made on `dev`; `main` and release tags stay unchanged until
reviewed release work. Development does not trigger package/release publication.
The public repository contains the Rust daemon, self-hosted dashboard, tests and
user documentation. Project/community websites and sponsored-node operations are
maintained separately in a private repository.

## Software and community network

- **rirrd.net** showcases the project, measured benchmarks and beta documentation.
- **irrd.tools** hosts public routing tools, the reviewed node directory and
  private node submissions. The first node is in the UK, hosted by HYEHOST LLC.
- The Rust daemon and `web/dashboard/` run independently. Running your own instance
  needs neither a community account nor a directory listing.

Start with [self-hosting](docs/operations/GETTING_STARTED.md),
[API commands](docs/operations/API_COMMANDS.md),
[BIRD/FRR integration](docs/operations/ROUTER_INTEGRATION.md).
RIRRd focuses on IRR hosting; cryptographic RPKI validation belongs to a separate
validator chosen by the operator.

## Contributions and AI disclosure

RIRRd is built with AI assistance. Human and AI-assisted contributions are welcome,
with review, reproducible tests and clear provenance. Code can be used, modified
and redistributed under MIT. Authorship does not establish correctness or security.
See [CONTRIBUTING](CONTRIBUTING.md), [AI disclosure](docs/AI_DISCLOSURE.md) and
[SECURITY](SECURITY.md). Report beta bugs using
[GitHub Issues](https://github.com/RIRRd/RIRRd/issues) for reproducible bugs and feature requests.

## Dashboard and router integration

With `http_listen` configured, open `/ui/` on that loopback listener. The bundled
read-only dashboard queries the same local WHOIS/GraphQL APIs, shows source and
replication status, and displays optional `[node]` name/location/sponsor labels.
No Node process or external page assets are required. Missing freshness remains
unreported; a configured source is not a freshness claim. Account/update UI is
not implemented. The public community service hosts a real, read-only registry at irrd.tools.
Its node directory and submission service are deployed separately from the daemon.

Start with [the small-host profile](examples/small-host/irrd.toml),
[small-host operation](docs/operations/SMALL_HOSTS.md), and the
[tested BIRD/FRR guide](docs/operations/ROUTER_INTEGRATION.md).

## Automated verification

Every push/PR runs the core matrix, independent protocol/recovery contracts,
isolated website checks, source coverage and dependency audit. Reports include
individual JSON/JUnit outcomes and retained failure logs. Short RPSL/NRTMv3 fuzz
runs and container smoke checks also run per push/PR. Read [the testing guide](docs/TESTING.md)
or inspect [the recorded verification page](https://rirrd.net/testing/).

```sh
python3 tools/testing/run.py --profile core --out /tmp/rirrd-core
python3 tools/testing/run.py --profile web --out /tmp/rirrd-web
```

[Core CI](https://github.com/RIRRd/RIRRd/actions/workflows/ci.yml) ·
[RPSL/NRTMv3 fuzzing](https://github.com/RIRRd/RIRRd/actions/workflows/fuzz.yml) ·
[Pinned upstream oracle](https://github.com/RIRRd/RIRRd/actions/workflows/upstream-oracle.yml)

## Install a Linux beta

[Release packages](https://github.com/RIRRd/RIRRd/releases) support Linux x86-64
and ARM64. See [installation and configuration](docs/operations/GETTING_STARTED.md).
The checked-in `tools/install.sh` verifies the release checksum, supports
`--download-only`, and leaves service startup to the operator.

## Local evaluation

Install stable Rust with the platform's native linker and C compiler for the TLS dependency, then build and run:

```sh
cargo build --locked
cargo run --locked -- --config examples/irrd.toml config check
cargo run --locked -- --config examples/irrd.toml import snapshot --source EXAMPLE --file examples/example.rpsl
cargo run --locked -- --config examples/irrd.toml database verify
cargo run --locked -- --config examples/irrd.toml server
```

From another terminal, use a TCP client to send a query terminated by LF or CRLF. For example, with netcat:

```sh
printf '!mroute,192.0.2.0/24AS64496\n' | nc 127.0.0.1 4343
```

| WHOIS request | Behaviour |
|---|---|
| Bare primary key, `!m<class>,<key>` | Case-insensitive exact keys. `!m` returns the first result in source priority order and accepts legacy space or hyphen separators in route keys. |
| `!r<prefix>[,o|l|L|M]`, `-x`, `-l`, `-L`, `-M` | Exact routes, exact origins, nearest strict cover, all covers including exact, and strict descendants. |
| `!g<asn>`, `!6<asn>` | Distinct IPv4 and IPv6 prefixes for an origin. |
| `!o<maintainer>`, `-i <field> <value>` | Maintainer or supported inverse-attribute lookup. |
| `!i<set>[,1]`, `!a[4|6]<as-set>` | Direct or recursive members, or originating routes for an AS-set. |
| `!!`, `-k`, `!q` | Start a persistent connection or close it. Each session has an idle deadline and a 1,024-line budget. Use `!t<seconds>` to set its idle deadline within the configured ceiling. |
| `!s<source-list>`, `!s-lc`, `!e<set-list>`, `!e-lc` | Select and inspect query sources or recursive set exclusions for the current connection. |
| `-s <source-list>`, `-a`, `-V <client>` | RIPE-style source selection, source reset and client identification. |
| `-q sources` | List configured sources, retained NRTMv3 journal bounds and whether the peer may mirror each source. |
| `!v`, `!n<client>` | Report version or acknowledge client identification. |

Bare, route and inverse lookups return at most 16 objects. A single-AS origin lookup returns at most 4,096 distinct prefixes. Set expansion has separate traversal limits. Requests exceeding a limit fail as a whole. The mirror parser validates record structure and identity. Indexing retains valid optional lookup values and reports malformed values that it skipped.

| Configuration | Purpose |
|---|---|
| `database` | redb file. A relative path is resolved against the TOML file's parent. |
| `database_cache_mib` | redb cache budget for daemon and offline commands; default 64 MiB, range 1–65536. This is not a total process-memory bound. |
| `worker_threads` | Optional Tokio runtime worker count, 1–64. Omit to use automatic CPU detection. |
| `max_blocking_threads` | Optional blocking-worker cap, 1–256. Omit for the runtime default. |
| `sources` | Ordered allowlist for imports and WHOIS queries, with 1–256 distinct uppercase names. |
| `listen` | TCP socket address. The example binds IPv4 loopback on port 4343. |
| `http_listen` | Optional loopback HTTP address for GraphQL and HTTP WHOIS. |
| `max_connections` | Concurrent client limit, 1–4096. |
| `max_connections_per_ip` | Optional concurrent WHOIS limit per client IP, at most the global limit. |
| `connection_limit_exempt_ips` | Optional list of up to 256 exact IPv4 or IPv6 addresses exempt from the per-IP limit. The global limit still applies. |
| `io_timeout_seconds` | Deadline per read, lookup or write, 1–300 seconds. |
| `max_snapshot_bytes` | Uncompressed input byte limit, 1 byte–64 GiB. Individual records are limited to 64 KiB. |

Import accepts local UTF-8 RPSL in plain text or gzip. Each successful import replaces the named source atomically. Empty, malformed, duplicate or wrong-source input fails the import. Stop the local daemon before running import, export or verification commands because redb takes exclusive process ownership of the database file. The verifier checks stored identities against both indexes.

To export one source, use a private directory and a new filename:

```sh
cargo run --locked -- --config examples/irrd.toml export snapshot --source EXAMPLE --file private/example.rpsl --include-sensitive
```

Export reads one committed view, writes raw objects to a temporary file, then publishes it without overwriting an existing file. You can import the snapshot into a new database configured for that source. This source-level round trip carries the stored RPSL, with blank object separators added as needed. Use the full database backup below when you need replication and recovery metadata.

Use the example on a trusted local machine. Keep database and snapshot files private because stored raw text can contain maintainer credentials. Public WHOIS rendering removes `auth` and supplied `rpki-ov-state` fields.

## Full database backup and restore

Stop the daemon and use a private directory for the backup and restored database. The backup contains raw credential-bearing RPSL, indexes, journals, protected names and replication state. The archive includes a version and a SHA-256 checksum. Restore validates the archive and database before publishing the restored file. Both commands require a new destination.

```sh
rust-irrd --config /etc/rust-irrd/irrd.toml database backup --file private/registry.rirrd
rust-irrd --config private/recovery.toml database restore --file private/registry.rirrd
rust-irrd --config private/recovery.toml database verify
```

Set the recovery configuration's `database` to the new file path. Preserve the configuration and external policy files separately, and use a release compatible with the stored schema.

## HTTP WHOIS

All private HTTP routes require one valid Host header naming `localhost` or the configured listener IP. Unexpected/missing/duplicate hosts and external absolute-form authorities return 400 before handling requests. IPv6 authorities use brackets. This complements the required loopback binding.

With `http_listen` enabled, send a URL-encoded WHOIS command in the `q` parameter to `GET /v1/whois/`. The endpoint returns plain text for matches and HTTP 204 for no result. NRTMv3 journal downloads require TCP and the configured peer ACL.

## GraphQL

Set `http_listen = "127.0.0.1:8080"` in TOML. Send JSON by POST to `/graphql/` with a `query` field. The implemented subset accepts bounded exact-key queries through `rpslObjects`, prefix queries through `asnPrefixes` and `asSetPrefixes`, set expansion through `recursiveSetMembers`, and configured-source and replication metadata through `databaseStatus`. The optional `rootSource` argument selects the root independently of the source order used for nested set lookups. For example:

```graphql
{
  rpslObjects(rpslPk: ["192.0.2.0/24AS64496"], objectClass: ["route"]) {
    rpslPk
    objectClass
    source
    objectText
    ... on RPSLRoute { origin }
  }
}
```

## Index upgrades and source purge

On opening an existing registry database, the server adds missing lookup indexes in one transaction. Back up the database before upgrading. To roll back the binary, restore a pre-upgrade copy before importing or purging sources.

Stop the daemon before purging a source. Use `source purge --source EXAMPLE --yes` with the same configuration to remove that source's objects and their index entries. A repeated purge succeeds. Remove the source from configuration separately if it should no longer be selectable. Purge is logical removal from the registry and database indexes.

Run `database compact --yes` with the daemon stopped to reclaim free storage where the database engine can do so. The command verifies application indexes and journal history before and after compaction and prints the before and after file lengths.

## Offline NRTMv3 mirroring

Import the source's full snapshot first and record its published serial. Stop the daemon before applying a transcript or syncing with a peer. For a snapshot whose serial was 41, use one of the following:

```sh
rust-irrd source apply-nrtm3 --source EXAMPLE --base-serial 41 --file updates.nrtm
rust-irrd source sync-nrtm3 --source EXAMPLE --base-serial 41 --endpoint 127.0.0.1:8043
```

Subsequent updates use the committed serial automatically. Run `rust-irrd source status --source EXAMPLE` to read it. A full snapshot replacement or purge clears the recorded serial.

This client profile accepts finite, unfiltered version 3 responses with explicit contiguous serials. It commits object changes, indexes and the last serial together after validating the complete response. Filtered streams can legally contain serial gaps and are outside the supported profile. Remote sync uses plaintext TCP with an explicitly supplied peer IP and port. Use a trusted network or authenticated tunnel for replication traffic.

The registry retains up to 4,096 recently applied operations per source. To serve those records over WHOIS, list the trusted peer addresses for each source:

```toml
[nrtm3_serve]
EXAMPLE = ["127.0.0.1", "::1"]
```

Peers can request `-g EXAMPLE:3:42-LAST` or give an explicit end serial. Responses allow at most 1,024 operations and 4 MiB of raw object text per request. Use an explicit range when catching up in chunks. The server checks the source-specific peer list independently of the connection's WHOIS source selection. Journal text uses the same credential redaction as public WHOIS. Snapshot replacement and source purge clear retained journal history.

Authoritative submissions add local operations in the same transaction as the registry change. `source status` reports their cursor as `local_serial`. A trusted snapshot replacement or purge creates a gap in the serial history so peers requesting across it must reload a snapshot. To bootstrap a peer, use `export snapshot --source EXAMPLE --public --file example.rpsl` and give the peer the `serial` reported by the command along with that file.

After restoring an older whole database backup, keep mirroring disabled while rebuilding a previously published source. Reimport a trusted raw snapshot with `--authoritative-restore --serial-floor` set to the last serial published before recovery. The next snapshot carries a serial above that floor.

Enable scheduled NRTMv3 for an already imported non-authoritative source:

```toml
[nrtm3_sync.EXAMPLE]
endpoint = "127.0.0.1:8043"
interval_seconds = 300
base_serial = 41
```

The first update uses the snapshot's base serial. Subsequent attempts read the committed serial. The worker stages each response privately before acquiring the write gate, retries on the configured interval, and publishes only complete, valid transcripts. Each scheduled response is capped at 64 MiB and the peer deadline is capped at 15 seconds.

## NRTMv4 verification and mirroring

Provide an independently obtained P-256 public key in SPKI PEM format, the compact ES256 notification, and optionally a local snapshot or delta:

```sh
rust-irrd source verify-nrtm4 --source EXAMPLE \
  --public-key peer.pem --notification update-notification-file.jose \
  --snapshot snapshot.json.gz
```

Use `--delta updates.json.gz --delta-version 42` to select a signed delta reference. Verification follows the pinned IRRd 4.5.4 profile and checks the signed metadata, timestamp, source, session, version, SHA-256 hash and JSON Text Sequence framing.

To mirror from local files, stop the daemon and use `source apply-nrtm4`. Supply `--public-key` on the first application only. Repeat `--file` in the order required by the signed plan, starting with the snapshot if the plan requires one, then deltas in ascending version order:

```sh
rust-irrd source apply-nrtm4 --source EXAMPLE \
  --public-key peer.pem --notification update-notification-file.jose \
  --file snapshot.json.gz --file delta.json.gz
```

Objects, indexes and trust state commit together. Later applications use the persisted key and support an authenticated successor key and a new session snapshot. Old sessions and keys are rejected. Supply no files when the notification is already current. Source status shows the NRTMv4 session and version. Generic purge, snapshot replacement and NRTMv3 updates reject NRTMv4-managed sources to preserve their replay protection.

For a one-shot HTTPS sync, stop the daemon and run:

```sh
rust-irrd source sync-nrtm4 --source EXAMPLE --public-key peer.pem \
  --notification-url https://mirror.example.net/nrtm4/update-notification-file.jose
```

Set the URL to a trusted peer. TLS uses the WebPKI root set. The client rejects redirects and fetches only signed relative references on the same HTTPS origin, after authenticating the notification. A sync processes up to 512 referenced files within the configured byte limits and a 600-second default download budget. Use `--timeout-seconds` to set that budget. Subsequent syncs use the persisted signing key. Online scheduling and NRTMv4 server publication remain open.

## Authoritative submissions

Set `authoritative_sources = ["EXAMPLE"]` alongside `sources` in TOML. Bootstrap operator-managed records and maintainers through a trusted snapshot using `import snapshot --authoritative-restore`. This flag explicitly permits full source replacement. The NRTMv3 client rejects authoritative sources.

Stop the server and run `rust-irrd update --source EXAMPLE --file changes.rpsl`. Pipe one password per line to standard input. Use `--delete` when every record in the file is a deletion. The supported write profile covers person, role, organisation, route, route6, AS-set, route-set, filter-set, peering-set, rtr-set, inet-rtr, aut-num, AS-block and domain creation, modification and deletion, and existing-maintainer changes. Operators bootstrap the first maintainer. AS-blocks can then be managed through authenticated updates. Domain objects require strong admin-c, tech-c and zone-c references and store delegation descriptions without changing DNS. New autonomous system records require authentication against the narrowest covering AS-block in the same source. Missing or equally specific overlapping AS-blocks cause creation to fail. Validation of aut-num identities and route origins rejects AS0, AS23456, AS65535 and AS4294967295 and accepts private-use ASNs. Authentication requires bounded `BCRYPT-PW` verification. An update requires a valid maintainer for both the existing and proposed record. A maintainer update also verifies the proposed credentials. Strong references are checked and can prevent deletion. New AS-, route-, filter-, peering- and router-sets require a leading ASN in their names. Creation also requires authentication against the same-source parent `aut-num` when it exists.

Route creation also checks the same-source parent when one exists. Exact or smallest enclosing address allocations take precedence over the closest strict covering route. Ambiguous equally specific parents reject creation. The authoritative parser requires unambiguous CIDR syntax and rejects AS0, default routes and unspecified host routes. Aggregate policy attributes are outside the supported profile.

Optionally set `authoritative_roa_file = "roas.json"` to reject incoming routes whose origin is invalid against a trusted, validator-produced feed. The daemon loads the file at startup. `authoritative_roa_exempt_sources` can exempt selected authoritative sources. Configure `[authoritative_scope]` with `prefixes`, `asns` or both to reject overlapping routes and excluded origin ranges. Its `exempt_sources` applies only to scope. These policies apply to new and modified authoritative routes. They leave existing and mirrored query results visible.

The submission applies atomically. Run `source history --source EXAMPLE` offline to read recent committed identities and actions. The history omits passwords, hashes and object bodies.

To migrate stored `MD5-PW` credentials, temporarily list the authoritative source in `legacy_md5_sources`. Existing MD5-Crypt hashes can then authenticate, but proposed maintainer records must use bcrypt. To rotate a self-maintained record, supply the current password and the password matching the proposed bcrypt hash. Remove the source from this list once migration is complete.

An existing maintainer can submit only `BCRYPT-PW DummyValue` auth placeholders
with one distinct password. After authenticating the stored maintainers, the
server replaces all placeholders with a fresh bcrypt hash at the library's
default cost (currently 12). Duplicate identical passwords count once. Multiple
distinct passwords, mixed real/dummy hashes and new-maintainer creation fail.
Replacement hashing shares the authentication work budget and occurs inside the
transaction before publication. The placeholder and plaintext password are never
stored. The normal CLI and HTTP paths have local and pinned-oracle coverage.

For local HTTP submission, set `http_authoritative_updates = true` alongside a loopback `http_listen`. Send JSON to `/v1/submit/` using POST to create or modify, or DELETE to remove. Use an `objects` list with one `object_text` per entry and a top-level `passwords` list. Each submission covers a single authoritative source. The server commits the batch and its audit records together. HTTP reports omit object text and secrets. Deleted contact and maintainer names are protected from reuse in their source.

## Development checks

See the [deployment examples](deploy/README.md) for Docker Compose and native systemd setup and the [native packaging guide](deploy/packages/README.md) for Debian and RPM builds on Linux x86-64 and ARM64. Linux CI defines container and package bootstrap, WHOIS and shutdown verification.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

The [100,000-object Linux synthetic comparison](docs/benchmarks/LINUX_SYNTHETIC_100K.md) records pinned-baseline import, query, memory and storage measurements. It is a bounded development checkpoint, separate from full parity and release qualification. The generator and validating load client are in [tools/bench](tools/bench/README.md).

## Experimental public test service

The live dashboard is https://irrd.tools/dashboard/ and read-only TCP WHOIS/bgpq4
uses `irrd.tools:43`. Inspect the dashboard's loaded sources, missing feeds and
snapshot times before testing. Seven sources follow NRTMv3 every sixty seconds; the remaining sources use daily snapshots.
Submissions and administration are not public; full parity and production
qualification remain incomplete. See the [router guide](docs/operations/ROUTER_INTEGRATION.md)
and [live deployment guide](docs/STATUS.md). Public IPv4/IPv6 and AS-set commands are in the [public query guide](docs/operations/PUBLIC_ROUTER_QUERIES.md).

`http_sources` optionally restricts HTTP WHOIS/GraphQL to a nonempty subset of
`sources`. TCP uses the full source list. This supports per-source distribution
requirements. The current public deployment exposes all nineteen loaded routing
sources, including ARIN, through TCP WHOIS, HTTP WHOIS and GraphQL.
