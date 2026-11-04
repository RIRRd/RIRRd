# Linux 100,000-object synthetic checkpoint

Measured 3 November 2026. These are local short synthetic measurements, not full parity, sustained-load qualification or a production-capacity claim. Compare configured implementations, not languages in isolation. Feature, validation and index coverage differ.

## Revisions and environment

- Performance candidate: `a7c7084db38335ce5bdd199465a5ba2650072e38`; release binary SHA-256 `b52b16bb1202861c3caa1d3021dbaf8a000e0e54d50b0ba34b005c1f8dc956f6`.
- IRRd v4.5.4: `8e8080dab744a348b7d0e6efcb53f403301dd075`; image `sha256:13fc702631c23e689a5530152dda869cf0c834e667be202662f673d064dc1f22`. PostgreSQL 18.0 and Redis 8.2.3 use the pinned oracle images; PostgreSQL uses image defaults.
- Before-optimization binary: source `25e7cdd0787eba8da20bf894d5cee7fb632e7d93`, SHA-256 `7aca3f3074682e59cfb1d5c2f81a4b4598d0c85ca8fbd30b2a9b03a857382aa5`. Its hash matches the earlier published five-run checkpoint.
- Debian 13 x86-64, KVM reports 24 CPUs (Intel Xeon E5-2683 v4 @ 2.10GHz), about 62 GiB RAM, ext4. Rust/cargo 1.99.1, release profile. Over 200 GiB disk was available before generating the 15.5 MB fixture.
- Servers, PostgreSQL and Redis: CPUs 0–15. Native load client: CPUs 16–23. Eight clients, IRRd eight WHOIS/eight HTTP workers; candidate connection cap 32 with eight active query clients.
- Static Caddy website remained active. No compile/fuzz/install work ran during timing. Registry services were loopback-only. No public-registry updates or third-party data imports.
- Fixture: 100,000 original synthetic objects; 50,000 aut-num, 25,000 route, 24,997 route6, three supporting objects. `15485046` bytes; SHA-256 `9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2`.

## Results

Import rows are medians of three fresh databases. Query rows give all three reused-connection 20-second rates, after three-second warmups. The latency columns are worst per-run quantiles, not global pooled quantiles.

| Measurement | RIRRd | IRRd 4.5.4 |
| --- | ---: | ---: |
| Import seconds | 8.44 | 126.01 |
| Import CLI CPU seconds | 5.80 | 93.78 |
| Peak import CLI RSS, MiB | 110.16 | 435.34 |
| Idle whole-stack process PSS, MiB | 37.74 | 2,497.08 |
| Persistent allocated database bytes, MiB | 97.32 | 571.17 |
| Whois queries/s, runs 1–3 | 22,666.98, 22,843.33, 22,135.32 | 1,402.41, 1,430.80, 1,414.00 |
| Whois worst per-run p95 ms | 0.559 | 7.106 |
| Whois worst per-run p99 ms | 0.719 | 8.761 |
| Whois maximum observed ms | 11.815 | 481.837 |
| Graphql queries/s, runs 1–3 | 9,868.87, 10,084.20, 10,089.10 | 439.25, 435.15, 460.73 |
| Graphql worst per-run p95 ms | 1.120 | 31.793 |
| Graphql worst per-run p99 ms | 1.390 | 40.862 |
| Graphql maximum observed ms | 13.608 | 300.415 |

All **2,834,489** measured responses across the 22 completed profiles validated successfully, including before/after checks. Zero measured errors. Initial failed harness attempts are excluded and explained below. The driver used at most 48.46% of its eight allocated logical CPUs, averaged over a profile. This is not a per-core saturation proof.

Three candidate import runs: 8.441 s, 5.528 s, 16.467 s. Reference: 124.523 s, 126.020 s, 126.013 s. Candidate import variance is retained, not trimmed.

### New-connection workloads

| Mode | RIRRd queries/s (p95 ms) | IRRd queries/s (p95 ms) |
| --- | ---: | ---: |
| Whois, single 10 s run | 11,199.29 (1.047) | 556.43 (17.531) |
| Graphql, single 10 s run | 6,560.05 (1.649) | 365.27 (34.377) |

### Optimization check

- before: 9,388.37, 9,754.30, 9,880.22 queries/s; median 9,754.30.
- current: 9,714.52, 9,673.48, 9,721.50 queries/s; median 9,714.52.

The three alternating 10-second GraphQL runs select common identity fields. The ranges overlap and do not establish a throughput improvement. Lazy parsing removes unnecessary allocations/work, but end-to-end gain was not demonstrated here. Other field selections remain unmeasured.

## Accounting and reproduction

- Imports accept exactly 100,000 objects in each fresh database. Host filesystem cache may be warm; no cache drop. Schema migration and process startup are outside CLI import timing. Peak CLI RSS excludes separate PostgreSQL/Redis processes.
- Memory is median of three summed idle process PSS samples after the query suite: candidate versus IRRd app/supervision plus PostgreSQL and Redis. Resident mmap pages count; unmapped filesystem cache does not. This is not peak memory.
- Storage is allocated redb blocks versus PostgreSQL cluster plus WAL, following a checkpoint; Redis persistence disabled. Images/logs excluded. Candidate apparent file size is separately recorded in JSON.
- Queries use a deterministic trace over a 50,000-ASN key space: exact-key WHOIS and GraphQL identity/source/class. Every measured response validates framing, key and source. GraphQL also checks one record/class and absence of errors. Active clients eight; reused connections recycle at 900 requests to remain below the candidate session bound. Individual requests have a five-second timeout.
- Main workloads alternate implementation order across three repetitions. New-connection workloads are single ten-second samples. All are closed-loop short workloads; no offered-load sweep, mixed updates or sustained saturation test.
- The initial driver sent `Host: localhost`; IRRd correctly returned 400 under its configured 127.0.0.1 allowlist. Direct POST checks confirmed 400/200 by Host, then the client was fixed and every query profile rerun. Three completed imports were retained. Failed-attempt logs remain private and no failed profile contributes success metrics.
- Raw JSON records exact binary/image/fixture hashes, CPU topology, import resource data, complete profile order, driver CPU use, latency distributions and memory samples. Runtime datasets, caches, logs and databases stay outside Git.

See [raw completed results](results/linux-100k-2026-11-03.json), [generator/controller instructions](../../tools/bench/README.md), and the [full benchmark methodology](METHODOLOGY.md). This run does not meet the full methodology’s five-minute/five-repetition, mixed-data and long-duration gates.

## Functional and security continuation

Host-guard source commit: `df37fef950183134ac60dd4121659457af640d06`. Its final normal-entry-point oracle rerun passed 39 TCP and 39 HTTP WHOIS cases, four policy/router value queries with 32 types, organisation values with nine types, 10 organisation/25 policy schema cases, and 47 HTTP authorisation decision cases.

- GraphQL filter-set, peering-set, rtr-set and inet-rtr scalar/list field values plus 32 field types were compared through normal HTTP with pinned IRRd. Supported organisation values and nine field types remain covered.
- Private HTTP now checks Host across GraphQL, WHOIS, submissions and fallback routes before handling requests. Unexpected/missing/duplicate hosts and external absolute-form authorities are rejected without echoing attacker text. Unit and live socket cases cover IPv4/IPv6 authority syntax, bad ports, trusted local names and normal requests.
- Deliberate private-interface policy: RIRRd accepts `localhost` and its configured listener IP; upstream accepts its configured `server.http.url` hostname. This is not a claim of configurable upstream virtual-host parity. HTTP listeners remain restricted to loopback. The performance binary above predates this additional Host guard; later security checks are reported separately.
- Local: cargo fmt, strict all-target Clippy, locked all-target tests (137 application unit, 13 integration, two validating-client tests) and RustSec audit (227 dependencies, 1,403-advisory database) pass. One ignored unit helper is invoked by the process-kill integration test.
- Linux native and ARM64 packaging/fuzz evidence from the [previous checkpoint](../STATUS.md) remains historical; it is not relabeled as a new campaign.

New passing CI run URLs remain unavailable with the deploy key alone. Repository/Actions read authentication is needed to verify them; local passes are not CI evidence. Main is unchanged. Full authentication/accounts, six remaining authoritative class profiles, NRTMv4 publication/scheduling/interoperability, live suppression, broader queries, migration/signing and sustained release qualification remain open.

## Verification commands

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo audit --db /var/lib/rirrd-dev/advisory-db --no-fetch
cargo build --locked --release
```

Start the isolated pinned oracle and a loopback candidate using
[the oracle workflow](../../.github/workflows/upstream-oracle.yml), then run:

```sh
python3 tools/oracle/compare_organisation_graphql.py
python3 tools/oracle/compare_policy_graphql.py
python3 tools/oracle/compare_policy_set_auth.py
python3 tools/differential_whois.py --reference 127.0.0.1:8043 --candidate 127.0.0.1:9043 --allow-filtered-auth-omission --normalize-attribute-padding
python3 tools/differential_whois.py --http --reference 127.0.0.1:8000 --candidate 127.0.0.1:9080 --allow-filtered-auth-omission --normalize-attribute-padding
```

The final VM log is `/tmp/rirrd-final-oracle.log`; local check logs are
`/tmp/rirrd-host-*-final.log`. Temporary candidate/oracle units were stopped after
verification. The temporary scale-bridge firewall rule was removed; SSH and web
input rules were preserved. The website deployment is tracked independently on
`codex/coming-soon`.
