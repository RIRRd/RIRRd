# Development review — 5 November 2026

Runtime: `6961d0b2a86baf024dfdc3fb65c73c9bdc0466da` (Rust changes in
`6106a79`, `a09c9ad` and `8bbe1e6`). Baseline:
`4337ad292bae2e7607c54ccd00a47f8d5649ce7d`. Candidate binary SHA-256:
`d6b9fbf1eff2cb2368df7bb1c45ee4af7d56895d7c34c65e452e90c3f9e55320`.
Main, releases and the production registry were not promoted by this work.
Full IRRd parity remains incomplete.

## Changes and limits

- Authoritative contact creation refuses a handle referenced by existing
  objects, even when no deletion tombstone exists. A synthetic baseline
  reproduction accepted this claim; the candidate and pinned IRRd reject it.
- An atomic batch can delete a contact/maintainer and all its referrers. A
  remaining reference, cross-source referrer or upserted referrer still blocks
  deletion. Authentication and full-transaction rollback remain required.
- WHOIS queries wait for their coherent read view without occupying a blocking
  worker. Mirror commits and HTTP updates similarly acquire their write guard
  before scheduling mutation work. Guard ownership is checked against the store.
  This ordering avoids a deadlock when queued readers hold guards while a
  running writer waits for them. Started workers retain guards after cancellation.
- Public object lookups check a **2 MiB raw-text/identity budget before copying**
  database values. Large records remain stored and exportable. This is per
  storage lookup, not a whole-process or whole-GraphQL-request ceiling. Parsing,
  response encoding, aliases and concurrent work need additional memory.
- Oversized object and set queries fail as a whole with a query-limit response;
  they do not return partial prefix lists or masquerade as internal errors.
- GraphQL `rpslObjects` supports `mntBy`, `memberOf`, `mbrsByRef`, `adminC`,
  `techC` and `zoneC`, together with keys, classes and sources. Values within
  one field are alternatives; different fields intersect in one read snapshot.
  The existing eight-object result limit remains. A 4,096-candidate scan budget
  bounds restrictive intersections; exceeding it returns an explicit error.
  Empty lists match nothing; null means no filter. Broader upstream filters and
  relationships remain unimplemented.
- WHOIS `!jSOURCE`, lists and `!j-*` expose retained journal/serial status.
  The optional export serial is omitted because it is not persisted. A configured
  but unloaded source reports `N:-`; upstream skips absent status rows. `!J`
  JSON status remains open.
- Verification counts already-sorted source/class groups without allocating two
  strings per object. Public snapshot export sanitizes borrowed text using the
  same redaction function as queries, avoiding a whole-record clone.

The database schema, data retention, durability, source scope and public admission
limits are unchanged. Snapshot records can be up to 8 MiB; authoritative records
remain limited to 64 KiB. Full-record public queries have the distinct budget
above; indexed key browsing can list identities without materializing their text.

## Verification

Local Linux gates: formatting, strict Clippy, locked all-target tests (**207
passed, one optional skip**), **61** external protocol/recovery contracts, **32**
report/tooling checks and **11** bundled-console browser cases. New tests exercise
single blocking-worker lock inversion, cancellation, wrong-store write guards,
large records, aggregate allocation limits, count corruption and byte-identical
redacted exports.

Pinned official IRRd **4.5.4**, commit
`8e8080dab744a348b7d0e6efcb53f403301dd075`, ran with isolated PostgreSQL and Redis.
The final binary matched **85 authoritative HTTP case groups**, **48 TCP** and
**48 HTTP WHOIS** cases, **12 inverse GraphQL** cases and six argument types,
plus the existing organisation and policy/router GraphQL comparisons. Six pinned
schema suites also passed. Fixtures are synthetic; no public registry updates
were sent. These comparisons establish only the exercised behavior.

Parser fuzzing completed 303,538 RPSL and 228,195 NRTMv3 executions without a
reported crash. The current dependency audit found zero vulnerabilities or
warnings against official RustSec snapshot
`26fd2d43f7280a1fb925a3743ec463017afe7119`. Direct Git/archive retrieval was
unavailable locally, so its changed blobs were obtained through GitHub's official
Git objects API; the complete resulting tree matched
`6cec6aa884aba1df6d5c62888f21760f4bd1d1de` before the no-fetch audit. CI performed
its own dependency audit.

Passing CI at the exact runtime commit:

- [Core Linux x86-64/ARM64, Windows, coverage, browser and audit](https://github.com/RIRRd/RIRRd/actions/runs/42007358789)
- [Pinned upstream oracle](https://github.com/RIRRd/RIRRd/actions/runs/42007359041)
- [Router and console integration](https://github.com/RIRRd/RIRRd/actions/runs/42007359054)
- [Deployment smoke](https://github.com/RIRRd/RIRRd/actions/runs/42007358672)
- [Parser fuzzing](https://github.com/RIRRd/RIRRd/actions/runs/42007358706)
- [Publication hygiene and secret-detection canaries](https://github.com/RIRRd/RIRRd/actions/runs/42007358875)

The hygiene check initially flagged the refactored Rust private-key detection
expression. Its exception matches that exact source expression only; general
private-key/API-key rules and deliberate-secret canaries remain active.

## Remaining work

| Area | Still open |
| --- | --- |
| Authoritative data | Complete schema/reference semantics; authoritative `inetnum`, `inet6num`, `key-cert` and `irt` workflows. All 19 classes remain available for mirrored ingestion and bounded queries. |
| Authentication and accounts | OpenPGP, scoped tokens, account lifecycle, override/suspension, maintainer provisioning, mail submission and notifications. |
| Mirroring and recovery | NRTMv4 publication/scheduling/rollover/retention, bidirectional qualification, broader NRTMv3/reseed failure cases and sustained restart/recovery testing. |
| Queries and policy | Full GraphQL filters/relationships/journals, source aliases, `!J`, live scope and route-preference visibility/journal transitions. |
| Resource use | Synchronous mirror fetching can occupy a blocking worker until timeout; a one-worker pool can delay queries. Further object-table packing/compression needs representative measurements and a compatible migration plan. |
| Operations and release | Config reload/migration, sustained mixed-load and full-catalog small-host qualification. Existing green packaging/platform checks do not establish those properties. |

RPKI validation and RTR are deliberately outside the current IRR-only scope.
The refreshed [upstream inventory](IRRD_ISSUE_BACKLOG.md) records separate issue
and release observations; it does not change the pinned compatibility baseline.

## Measurements

[Raw samples, fixture hashes and binary hashes](../benchmarks/results/app-review-2026-11-05.json)
retain every completed measured repetition. These are separate workloads on the
same Debian 13 VM with 24 online logical CPUs and about 63 GiB usable RAM.
Public services remained active. No compilers or fuzzers ran during timing.
They are not a minimum-host or maximum-capacity qualification.

### Large rejected queries

Seventeen original synthetic filter-set records, each about 1.5 MB, share a
maintainer. Both binaries return the same `F Too many results` response. Three
fresh processes per binary alternate order, with 32 validated queries each,
an 8 MiB database cache and CPU affinity restricted to one logical CPU.

| Metric | Previous | Candidate |
| --- | ---: | ---: |
| Median process peak RSS | 45.35 MiB | 17.16 MiB |
| Worst per-run p95 | 109.29 ms | 2.11 ms |
| Validated rejections | 96 | 96 |

This is about 62% less peak process memory for **oversized rejected queries**,
not a successful-query throughput increase. All 17 records remain in the database
and verify after the runs. RSS excludes unmapped filesystem cache.

Reproduce with the normal binaries; fixtures and results must stay outside Git:

```sh
python3 tools/bench/check_query_budget.py \
  --before /private/baseline-rust-irrd --candidate target/release/rust-irrd \
  --before-commit 4337ad292bae2e7607c54ccd00a47f8d5649ce7d \
  --candidate-commit 6961d0b2a86baf024dfdc3fb65c73c9bdc0466da \
  --out /private/query-budget
```

### Verification and export

Six alternating measured runs per binary follow one warm-up, against one
immutable 300,000-record synthetic database, three sources and two classes,
using a 64 MiB cache. Verification median fell from **1.850 to 1.555 seconds**
(about 16%). The observed ranges were 1.534–2.140 and 1.451–1.629 seconds.
Peak RSS was essentially unchanged at about 69 MiB.

Exporting 100,000 records took medians of **0.225 and 0.223 seconds**; neither
export speed nor peak RSS improved meaningfully in this fixture. Every export
had the same SHA-256. The removal of the extra record clone is a code-level
allocation improvement, not evidence of faster ordinary exports.

The fixture contains sources TEST, OTHER and FINAL. For each source and each
integer `n` in `0..50000`, write an `aut-num: AS{64496+n}` object and an
`as-set: AS-TEST{n}` object, each followed by
`remarks: synthetic storage audit fixture`, its `source` and a blank separator.
Import each source once. Alternate the normal `database verify` and
`export snapshot --source TEST --public --file NEW_PATH` commands between the
saved binaries. Remove only the generated output between exports, and compare
all output hashes. The raw report retains each CLI result and elapsed/RSS sample.

### Normal queries on one CPU

The original 100,000-object synthetic PERF fixture is unchanged, SHA-256
`9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2`.
Each profile uses one async worker, four blocking workers, a one-CPU/100% quota
and a 128 MiB service limit. Four validating load clients run on separate CPUs,
reuse connections and query 50,000 ASN keys. Three ten-second runs follow
three-second warm-ups; profile order rotates. File caches can be warm.

| Metric | Previous, 64 MiB cache | Candidate, 64 MiB | Candidate, 16 MiB |
| --- | ---: | ---: | ---: |
| WHOIS median queries/s | 11,080 | 10,820 | 10,465 |
| WHOIS worst per-run p95, ms | 0.744 | 0.757 | 0.724 |
| GraphQL median queries/s | 3,894 | 3,837 | 3,632 |
| GraphQL worst per-run p95, ms | 2.211 | 2.117 | 2.341 |
| Idle process PSS, median MiB | 33.08 | 33.07 | 24.91 |
| Fresh import median seconds | 9.35 | 6.45 | 10.21 |
| Database file length, MiB | 129.01 | 129.01 | 129.01 |

All **1,296,605** measured replies validated with zero errors. At the same cache
budget median WHOIS and GraphQL throughput were approximately **2.4% and 1.5%
lower**, with overlapping run-to-run ranges. This pass does not establish faster
normal queries. Idle PSS is unchanged; the smaller cache remains a memory/speed
trade-off. Import wall times vary with storage/cache conditions; do not infer a
general import speedup from this three-run result. Allocated database bytes are
about 97.3 MiB in all profiles, within small allocation variation. No database file-size
reduction is claimed for this pass. The executable grew from 12,355,744 to
12,442,072 bytes (about 0.7%) with the added features and guards.

Reproduce with `tools/bench/generate_fixture.py` and
`tools/bench/run_small_host.py`, passing the saved baseline, candidate commits,
`--before-cache-mib 64`, and unused loopback ports. The helper records every
import, query sample, RSS/PSS observation and allocated/apparent disk size.

### Pinned upstream smoke comparison

The same original 12-object DIFFERENTIAL fixture was loaded into both engines.
Three alternating runs issue 2,000 exact-key TCP requests per engine with four
clients, opening a new connection per query. All 12,000 measured replies validate.
Median throughput was **1,563 queries/s for RIRRd** and **96 for IRRd** in this
specific connection-per-query smoke. This small fixture and connection behavior
cannot establish general engine capacity, sustained throughput or full-dataset
resource ratios. It is separate from the reused-connection measurements above.
The oracle used one WHOIS worker and the pinned image and commit recorded in the
raw report; PostgreSQL/Redis were isolated and removed after verification.

The raw footprint record includes reference service cgroup memory (including
cache and helper processes), candidate RSS, and persistent bytes. Those memory
scopes differ and must not be divided into a comparative memory-saving claim.
The previously verified [28-to-20 GiB full-catalog repack](../benchmarks/STORAGE_FULL_2026_11_05.md)
remains the actual full-dataset storage reduction. No database encoding or live
catalogue data was changed for these optimizations.
