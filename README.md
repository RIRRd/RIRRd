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

The supported TCP requests are a bare case-insensitive primary key, `!m<class>,<key>`, `!g<asn>` for IPv4 origins, `!6<asn>` for IPv6 origins, `!r<prefix>` for exact route matches, `!r<prefix>,o` for their origins and `!v`. `!m` uses configured source order, case-insensitive keys matching the pinned upstream code and legacy space/hyphen handling for route composite keys. Each connection processes one request. Bare and exact route lookups return at most 16 matches, and origin lookups return at most 4,096 distinct prefixes. A request exceeding either limit receives an error. `!m` returns a single result or the IRRd `D` frame. The parser accepts a subset of IRRd RPSL and validates record structure and identity.

| Configuration | Purpose |
|---|---|
| `database` | redb file. A relative path is resolved against the TOML file's parent. |
| `sources` | Ordered allowlist for imports and WHOIS queries, with 1–256 distinct uppercase names. |
| `listen` | TCP socket address. The example binds IPv4 loopback on port 4343. |
| `max_connections` | Concurrent client limit, 1–4096. |
| `io_timeout_seconds` | Deadline per read, lookup or write, 1–300 seconds. |
| `max_snapshot_bytes` | Uncompressed input byte limit, 1 byte–64 GiB. Individual records are limited to 64 KiB. |

Import accepts local UTF-8 RPSL in plain text or gzip. Each successful import replaces the named source atomically. Empty, malformed, duplicate or wrong-source input fails the import. Stop the local daemon before running import, export or verification commands because redb takes exclusive process ownership of the database file. The verifier checks stored identities against both indexes.

To export one source, use a private directory and a new filename:

```sh
cargo run --locked -- --config examples/irrd.toml export snapshot --source EXAMPLE --file private/example.rpsl --include-sensitive
```

Export reads one committed view, writes raw objects to a temporary file, then publishes it without overwriting an existing file. You can import the snapshot into a new database configured for that source. This source-level round trip carries the stored RPSL, with blank object separators added as needed. Source serials, recovery metadata and a versioned backup format are future work.

Use the example on a trusted local machine. Keep database and snapshot files private because stored raw text can contain maintainer credentials. Public WHOIS rendering removes `auth` and supplied `rpki-ov-state` fields.

## Development checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```
