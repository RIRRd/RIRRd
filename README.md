# Rust IRRd

This development branch contains an offline snapshot importer, a transactional embedded registry and a limited TCP WHOIS listener. The compatibility baseline is IRRd v4.5.4 at commit `8e8080dab744a348b7d0e6efcb53f403301dd075`. See the [compatibility matrix](docs/compatibility/IRRD_FEATURE_MATRIX.md) and [development checkpoint](AUTONOMOUS_PROGRESS.md) for release blockers and evidence.

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

Set `http_listen = "127.0.0.1:8080"` in TOML. Send JSON by POST to `/graphql/` with a `query` field. The implemented subset accepts bounded exact-key queries through `rpslObjects`, with optional object classes and sources. For example:

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

## Development checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```
