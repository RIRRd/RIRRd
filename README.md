# Rust IRRd

This development branch contains a transactional embedded registry, local snapshot and incremental mirroring commands, a TCP WHOIS listener and an optional read-only GraphQL service. The compatibility baseline is IRRd v4.5.4 at commit `8e8080dab744a348b7d0e6efcb53f403301dd075`. See the [compatibility matrix](docs/compatibility/IRRD_FEATURE_MATRIX.md), [upstream issue backlog](docs/compatibility/IRRD_ISSUE_BACKLOG.md) and [development checkpoint](AUTONOMOUS_PROGRESS.md) for release blockers and evidence.

## Local evaluation

Install stable Rust with the platform's native linker, then build and run:

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
| `!!`, `-k`, `!q` | Start a persistent connection or close it. Each session has an idle deadline and a 1,024-line budget. |
| `!s<source-list>`, `!s-lc`, `!e<set-list>`, `!e-lc` | Select and inspect query sources or recursive set exclusions for the current connection. |
| `!v`, `!n<client>` | Report version or acknowledge client identification. |

Bare, route and inverse lookups return at most 16 objects. A single-AS origin lookup returns at most 4,096 distinct prefixes. Set expansion has separate traversal limits. Requests exceeding a limit fail as a whole. The mirror parser validates record structure and identity. Indexing retains valid optional lookup values and reports malformed values that it skipped.

| Configuration | Purpose |
|---|---|
| `database` | redb file. A relative path is resolved against the TOML file's parent. |
| `sources` | Ordered allowlist for imports and WHOIS queries, with 1–256 distinct uppercase names. |
| `listen` | TCP socket address. The example binds IPv4 loopback on port 4343. |
| `http_listen` | Optional loopback HTTP address for read-only GraphQL. |
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

Export reads one committed view, writes raw objects to a temporary file, then publishes it without overwriting an existing file. You can import the snapshot into a new database configured for that source. This source-level round trip carries the stored RPSL, with blank object separators added as needed. Source serials, recovery metadata and a versioned backup format are future work.

Use the example on a trusted local machine. Keep database and snapshot files private because stored raw text can contain maintainer credentials. Public WHOIS rendering removes `auth` and supplied `rpki-ov-state` fields.

## GraphQL

Set `http_listen = "127.0.0.1:8080"` in TOML. Send JSON by POST to `/graphql/` with a `query` field. The implemented subset accepts bounded exact-key queries through `rpslObjects`, with optional object classes and sources, and prefix queries through `asnPrefixes` and `asSetPrefixes`. For example:

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

## Offline NRTMv4 verification

Provide an independently obtained P-256 public key in SPKI PEM format, the compact ES256 notification, and optionally a local snapshot or delta:

```sh
rust-irrd source verify-nrtm4 --source EXAMPLE \
  --public-key peer.pem --notification update-notification-file.jose \
  --snapshot snapshot.json.gz
```

Use `--delta updates.json.gz --delta-version 42` to select a signed delta reference. Verification follows the pinned IRRd 4.5.4 profile and checks the signed metadata, timestamp, source, session, version, SHA-256 hash and JSON Text Sequence framing. This command uses local files and a trusted key supplied by the operator. Database application, persistent replay protection, automated key rotation and HTTPS mirroring remain open.

## Offline route origin validation

Supply JSON exported by a trusted RPKI validator with a top-level `roas` list. Each entry specifies `prefix`, `asn` and `maxLength`. The command returns `VALID`, `INVALID` or `NOT_FOUND`.

```sh
rust-irrd rpki validate --roas validated-roas.json --prefix 192.0.2.0/24 --origin 64496
```

This command evaluates validated payloads locally. It does not currently annotate or suppress live registry results.

## Development checks

See the [deployment examples](deploy/README.md) for Docker Compose and native systemd setup. A separate Linux CI job defines container bootstrap, WHOIS and shutdown verification.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```
