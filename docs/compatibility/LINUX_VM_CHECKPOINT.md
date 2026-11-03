# Linux VM checkpoint — 3 November 2026

This is bounded compatibility evidence, not full parity or production readiness.
The development base is `bffd77a`, which was not an ancestor of the fetched main
(`ef0a31b`). Changes use `codex/linux-parity`; main was not modified.

## Implemented and locally verified

- `acb6cac`: authoritative organisation create/modify/delete through normal CLI
  and opt-in loopback HTTP. Required fields and multiplicity follow the pinned
  `RPSLOrganisation` schema. Wrong password, source denial, strong contact
  references, failed-batch rollback and secret-free feedback have integration tests.
- Organisation scalar/list GraphQL fields follow pinned introspection types.
  Live HTTP values and nine field introspection types match pinned upstream via
  `tools/oracle/compare_organisation_graphql.py`. Relationship resolvers, journal
  and updated fields remain open.
- The synthetic oracle fixture now has six objects. The 39 TCP and 39 HTTP
  cases include organisation hit/miss and IPv4/IPv6 nearest-cover, all-cover and
  more-specific boundaries. All matched the official pinned daemon. Framing is
  validated before the existing explicit auth-placeholder and attribute-padding
  allowances. Ten independent pinned organisation schema cases passed.
- Root GraphQL introspection exposed the same five query names. This does not
  establish full schema parity. The organisation type initially had 9 fields
  versus upstream's 22; eight scalar/list fields were added after that comparison.

## Commands and results

On Debian 13 x86-64, Rust 1.99.1, 8 CPUs, 31 GiB RAM:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
cargo audit --db /var/lib/rirrd-dev/advisory-db --no-fetch
cargo +nightly fuzz run rpsl /var/lib/rirrd-dev/fuzz-rpsl -- -max_total_time=60 -max_len=65537
```

The unchanged base passed 130 unit tests (one ignored process-crash helper invoked
by its parent test) and 10 integration tests. The organisation checkpoint passed
131 unit tests and 11 integration tests, including the subsequent GraphQL field
change. Strict Clippy passed. The dependency
audit passed for 227 dependencies against 1,403 advisories at RustSec DB commit
`6c5fedf8ff03c485ed674d8f941e0180f3f72d70`. The normal automatic advisory fetch
failed at a GitHub endpoint; a separately fetched, TLS-verified checkout supplied
the audit DB. The Linux fuzz smoke completed 1,066,347 executions in 61 seconds
without a crash. It exercises the RPSL fuzz target, not every daemon surface.

Native amd64 Debian and x86_64 RPM archives built. The Debian package passed
install, permissions, systemd startup, loopback WHOIS, graceful SIGTERM and database
verification. Its service is stopped and disabled. RPM installation and ARM64
runtime checks remain pending new CI; package building alone is not runtime evidence.

## Oracle provenance and isolation

IRRd v4.5.4: `8e8080dab744a348b7d0e6efcb53f403301dd075`.
Wheel SHA-256: `0ee7a0d64d8aef7c8b7ee0d2f7a24ff96c4ff1aa37b8d443cd2f3445ff939929`.
All 151 packaged Python files matched the pinned checkout after CRLF normalization.
Built image: `sha256:13fc702631c23e689a5530152dda869cf0c834e667be202662f673d064dc1f22`.
The checked-in Dockerfile was built with host networking to use working IPv6
outbound connectivity; runtime containers use a dedicated bridge with isolated
PostgreSQL and Redis. Only 127.0.0.1:8043/8000 are published. Candidate binds
127.0.0.1:9043/9080. No updates were sent to a public registry.

Runtime data, package files, generated fuzz corpus, pip freeze, logs and raw
measurement JSON are under `/var/lib/rirrd-dev/` or `/tmp/rirrd-*.log`, outside Git.
No large public dataset was imported. The public Caddy site remains independent.

## Comparative smoke

Same VM and six-object synthetic fixture, sequential reference then candidate,
20 warmups and 1,000 requests each, 8 client threads, new TCP connection per query.
Candidate code: `acb6cac`; reference commit and image as above. No concurrent
build or fuzz workload ran during the timed requests. This is one short sample,
with the checked-in oracle worker settings, not a controlled sustained-load study.

| Measurement | Pinned IRRd | RIRRd |
| --- | ---: | ---: |
| Successful queries/s | 116.32 | 1618.74 |
| p50 ms | 65.65 | 4.53 |
| p95 ms | 78.53 | 7.58 |
| p99 ms | 83.97 | 9.71 |
| Max ms | 347.57 | 26.71 |

Post-run Docker memory: reference 342.6 MiB, PostgreSQL 38.96 MiB, Redis 9.42 MiB.
Candidate systemd cgroup MemoryCurrent: 4,149,248 bytes. These are different
accounting scopes, not equivalent peak RSS measurements. PostgreSQL volume:
49,980,219 bytes; candidate database: 3,686,400 bytes. Persistent file sizes include
engine overhead; neither measurement establishes production sizing.

## External gates and remaining parity

The deploy key supports pushes. The unauthenticated private-repository Actions
API returns 404, so new passing CI run URLs cannot yet be verified. Authenticate
a GitHub CLI session with repository/Actions read access or connect GitHub with
those permissions. Do not send a token in chat. Existing historical CI links in
the matrix remain historical evidence. Local checks are not labeled as CI passes.

Remaining work includes the other authoritative classes and full strong-reference
rules, maintainer provisioning, OpenPGP/key-certs, scoped tokens, override,
suspension, accounts, mail/notifications, bidirectional upstream mirroring,
NRTMv4 publication/scheduling/retention/recovery, broader WHOIS and GraphQL parity,
live RPKI/scope/preference suppression, restart/upgrade/rollback qualification,
ARM64 and RPM runtime CI, migration, signing and sustained-load/security gates.
The 34 upstream issues, six PRs and four public advisories were refreshed from
official APIs; all open records remain mapped in the backlog. Latest release
remains v4.5.4. Full parity remains unfinished.
