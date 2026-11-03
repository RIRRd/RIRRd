# Upgraded Linux VM checkpoint — 3 November 2026

The VM now has 24 CPUs, 62 GiB RAM, Debian 13 x86-64 and Rust 1.99.1.
Both intended public addresses and the Caddy site survived the upgrade. Cargo
parallelism was raised from 8 to 24. Main is unchanged and all work uses codex/
branches. The repository remains private; the public comparison is authorised.

## Implemented

- `496f04c`: filter-set, peering-set and rtr-set authoritative CLI/HTTP profiles.
  They reuse old/new maintainer checks, same-source parent ASN authentication,
  strong contact references and atomic audit/journal publication. Router members
  are weak references; missing referenced routers/sets/maintainers do not invent
  a strong-reference requirement. Empty comma tokens follow pinned upstream.
- `d6d4d50`: inet-rtr authoritative updates, local ASN and alias validation,
  router-set membership, contact references and source isolation.
- `25e7cdd`: existing-maintainer BCRYPT-PW DummyValue replacement. Authentication
  uses stored maintainers first. One distinct password produces a fresh salted
  bcrypt hash; mixed real/dummy hashes and multiple distinct passwords fail.
  Hashing is charged to the shared authentication budget. Placeholders never
  enter storage. New maintainers still require administrator provisioning.
- Explicit amd64/arm64 packaging validates the selected ELF architecture before
  creating output and uses the matching cross strip tool for RPMs. CI includes
  negative architecture checks. Native automatic-target behavior is retained.

There are bounded authoritative profiles for 13 classes. Parsing/recognition of
all 19 mirror classes does not establish full authoritative validation parity.

## Correctness evidence

The pinned oracle remains IRRd v4.5.4 at
`8e8080dab744a348b7d0e6efcb53f403301dd075`, using the checksum-verified wheel
and image documented in [the earlier checkpoint](LINUX_VM_CHECKPOINT.md).

| Check | Observed result |
| --- | --- |
| Native Linux format / strict Clippy / locked tests | Passed; 134 unit tests and 13 integration tests. One ignored process-crash helper is invoked by its parent test. |
| Pinned TCP / HTTP WHOIS corpus | 39 / 39 cases matched on each transport. |
| Organisation parser / GraphQL | 10 schema cases; selected HTTP values and nine introspection field types matched. |
| Policy-set/router schema | 25 strict parser decisions matched pinned upstream. |
| Authoritative HTTP oracle | 47 cases checked through both real submission endpoints: wrong/owner/parent proofs, create/modify/delete, missing contacts, cross-source denial, maintainer placeholder replacement and credential redaction. |
| Dependency audit | Passed: 227 dependencies, 1,403 advisories at RustSec DB `6c5fedf8ff03c485ed674d8f941e0180f3f72d70`, refreshed with Git before the scan. |
| RPSL fuzzing | Two workers with seeds 42/43, 121 seconds each: 1,584,919 and 1,625,526 executions, zero crash artifacts. This exercises the RPSL target, not every daemon surface. |
| Native amd64 package | Debian/RPM built; Debian install, systemd startup, WHOIS, SIGTERM and verify passed. Original state/config backed up and state restored; service stopped and disabled. |
| ARM64 QEMU release profile | 134 unit and 13 integration tests passed with unchanged socket deadlines. |
| ARM64 QEMU debug profile | 134 unit tests and 12 integration tests passed. The maintainer dummy-update integration test exceeded the fixed five-second socket read deadline. This debug-emulation limitation remains recorded; it is not a native ARM64 CI pass. |
| ARM64 packages | Debian/RPM built with correct arm64/aarch64 metadata. Debian installed and passed CLI, loopback WHOIS, SIGTERM and database verify in a network-disabled, emulated Debian 12 container. |

Official upstream APIs were refreshed after these changes: latest release is
still v4.5.4; 34 open issues and six PRs remain mapped in the backlog, with no
unmapped open record.

The HTTP oracle has separate synthetic AUTH-TEST and AUTH-OTHER sources and a
loopback-only SMTP sink that discards notifications without delivery or content
logging. It compares the tested decisions and report counters, not the complete
upstream response/notification contract. Credentials/fixtures are synthetic.
Generated data, logs, databases, packages and fuzz corpora stay outside Git.

Representative commands (the full bootstrap is in the oracle workflow):

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
cargo audit --db /var/lib/rirrd-dev/advisory-db --no-fetch
python3 tools/oracle/compare_policy_set_auth.py
python3 tools/oracle/compare_organisation_graphql.py
# Run the parser check inside the pinned reference container:
docker compose -p irrd-454-oracle -f tools/oracle/compose.yaml exec -T reference python < tools/oracle/check_policy_set_schema.py
# ARM64 execution needs the cross compiler, sysroot and binfmt/QEMU described
# in deploy/packages/README.md:
cargo test --locked --release --target aarch64-unknown-linux-gnu --all-targets
bash deploy/packages/build.sh --arch arm64 target/aarch64-unknown-linux-gnu/release/rust-irrd dist-arm64
```

## Repeated comparison on the upgraded VM

The public page and [JSON results](https://rirrd.co.uk/comparison.json) report
five alternating runs on a fresh six-object oracle dataset. Candidate Rust code
is `25e7cdd0787eba8da20bf894d5cee7fb632e7d93`. IRRd uses eight WHOIS workers;
RIRRd has an eight-connection cap. Eight clients, 20 warmups and 1,000 exact-key
queries per implementation per run, each on a new TCP connection. No compilation
or fuzz work ran during timing. Caddy remained active. These are short synthetic
samples, not sustained-load, feature-equivalent or production qualification.

| Statistic | IRRd 4.5.4 | RIRRd |
| --- | ---: | ---: |
| Median per-run queries/s | 545.23 | 1,576.62 |
| Median of per-run p50 ms | 14.26 | 4.58 |
| Median of per-run p95 ms | 17.82 | 7.77 |
| Median of per-run p99 ms | 22.59 | 12.00 |
| Maximum observed ms across runs | 61.98 | 26.52 |
| Per-run queries/s range | 532.03–569.66 | 1,484.11–1,774.85 |

Post-run resource observations: reference Docker memory 483.7 MiB, PostgreSQL
66.04 MiB, Redis 9.895 MiB; candidate systemd cgroup current/peak were
7,213,056 / 11,386,880 bytes. Docker's displayed memory and systemd cgroup values
have different accounting scopes. They are not equivalent process peak RSS.
PostgreSQL volume was 49,798,931 bytes; candidate redb file 3,686,400 bytes.
These sizes include engine overhead and do not establish production sizing.
Binary/fixture hashes, resource configuration, timestamps, order and each run
are in the public JSON. The earlier 8-CPU, one-worker reference sample remains
historical context and should not be combined with this profile.

## Deployment and remaining gates

The comparison page is deployed from codex/coming-soon at `e51310f` through
`caddy.service`, with local HTML/CSS/JSON under `/var/www/rirrd-coming-soon/`.
HTTP redirects to HTTPS and Caddy manages the Let's Encrypt certificate. Only
SSH and the static website are public. Registry development/oracle listeners
use loopback or isolated networks and are stopped after verification.

External current-page probes returned HTTPS 200 with valid certificates and
the new values over IPv4 in Buffalo and IPv6 in Newcastle:
[IPv4](https://api.globalping.io/v1/measurements/2rHObJnmqgXZvO1sr00021R4Z),
[IPv6](https://api.globalping.io/v1/measurements/2eubGeVTghtVsGN1J00021R4Z).
No DNS changes or repository visibility changes were made.

Full parity remains incomplete. Open work includes remaining authoritative
classes and comprehensive reference rules, OpenPGP/key-certs, scoped tokens,
overrides/suspension, account and administrator workflows, mail/notifications,
NRTMv4 publication/scheduling/retention and bidirectional recovery/interoperability,
broader WHOIS/GraphQL/source-alias parity, live RPKI/scope/preference policy,
migration, signing, sustained load and independent security qualification.
Native ARM64 CI and RPM runtime checks remain open. New private Actions results
are not readable using the deploy key, so no new passing CI links are claimed.
An authenticated repository/Actions connection is needed to verify those runs.
