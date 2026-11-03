# Autonomous development checkpoint

## Baseline

IRRd v4.5.4, commit `8e8080dab744a348b7d0e6efcb53f403301dd075`, is the pinned compatibility oracle. The [feature matrix](docs/compatibility/IRRD_FEATURE_MATRIX.md) records the reference behaviour, requirements and evidence for each area. [Architecture decisions](docs/ARCHITECTURE.md) record the initial storage selection and threat model. [Benchmark methodology](docs/benchmarks/METHODOLOGY.md) defines the controls for comparative testing.

## Implementation

| Milestone | Checkpoint |
|---|---|
| 0 Research | Baseline, feature and RPSL schema inventory, standards links, threat model and benchmark design recorded. Storage trade-offs are qualitative pending measured evaluation and the inventory needs differential evidence. |
| 1 Foundation | CLI, validated TOML, structured JSON logs, process lifecycle, embedded store and Windows/Linux CI workflow. CI execution and audit results are separate verification gates. |
| 2 RPSL and storage | Structural parsing for 19 classes, canonical identities, bounded plain/gzip snapshot replacement, exact and origin indexes, and logical verification. Full attribute and reference validation, source journals and prefix/set/reverse indexes are open. |
| 3 Queries | Persistent TCP, source/exclusion state, exact/covering/covered prefixes, origin and inverse indexes, and direct and recursive set expansion. Optional loopback GraphQL serves bounded exact-object queries. Broader RIPE flag combinations, full GraphQL schema and upstream interoperability remain open. |
| 4 Mirroring | Local snapshot import/export and tested object round trip. Offline NRTMv3 applies finite contiguous transcripts and persists the serial and retained history atomically. Configured peers can retrieve bounded, credential-redacted ranges over TCP. Filtered-gap streams, NRTMv4, scheduling and upstream interoperability remain open. |
| 5–7 Registry services | Offline origin validation evaluates validator-produced ROA feeds. Authoritative writes, authentication, live RPKI/scope filtering, REST and admin UI remain open. |
| 8–10 Release | Initial local tests, RustSec scan, RPSL fuzz target, process-kill recovery test and synthetic smoke benchmark. Fuzz execution, host-crash and storage-failure recovery, broader security review, load tests, release packaging, production migration and readiness gates remain open. |

## Verification

- Query checkpoint on Windows passed 36 unit and 3 end-to-end tests, including per-IP admission, TCP pipelining and session isolation, prefix boundaries, inverse lookups, set expansion and credential-redacted GraphQL HTTP. Root dependency audit passed with 160 locked dependencies. An upstream-oracle job is configured for a Linux runner.
- NRTMv3 checkpoint on Windows passed 47 unit tests and four end-to-end tests. The new TCP/file test exercises rollback on a bad trailer, replay rejection, serial persistence and deletion via a loopback peer. Transport tests cover absolute deadlines and no-update replies.
- The offline ROV and GraphQL prefix checkpoint passed 53 unit tests and five end-to-end tests on Windows. It covers all three ROV states, dual-stack and AS0, GraphQL ASN/AS-set prefix queries, and a TCP per-IP exemption that preserves the global cap.
- The NRTMv3 server checkpoint passed 56 unit tests and five end-to-end tests on Windows. One Rust registry replayed the retained journal from a second Rust registry and then received a clean no-update reply. The test also checks source ACL denial and credential redaction.
- Non-root Docker/Compose and systemd deployment examples are in [deploy](deploy/README.md). Compose configuration, daemon configuration and workflow shell syntax passed local validation. Linux x86-64 and ARM64 CI and container smoke definitions are present. Image build, runtime container and systemd startup, native packages and release signing remain open.
- The [upstream issue inventory](docs/compatibility/IRRD_ISSUE_BACKLOG.md) tracks 34 open issues and six open pull requests. Added local coverage for parts of #349 and #608, and an explicit offline source purge for part of #952.
- Windows Rust 1.99.1 passed formatting, Clippy with warnings denied, 19 unit tests and one CLI/TCP integration test. One ignored helper is invoked by the process-kill test, which confirmed the last acknowledged snapshot and indexes survived termination during replacement.
- The RustSec 0.22.2 scan checked 83 dependencies in the root lockfile against 1,402 advisories and exited successfully. The released Windows executable was checksum-verified. The source build of the audit tool required a C compiler absent from this host.
- The exact-key differential harness passed 13 fixture cases in a Rust-to-Rust self-check. This exercises the harness. Upstream execution remains pending. The [Compose oracle](tools/oracle/README.md) passed configuration validation only.
- The isolated RPSL fuzz target type-checked. A scheduled Linux CI job is configured for libFuzzer. No full fuzz run or remote CI run has been observed.
- The [Windows synthetic smoke report](docs/benchmarks/WINDOWS_SMOKE.md) records three release-build repetitions and the raw results. At eight concurrent clients the mean was 10,903 queries/s with p95 0.88–1.14 ms. There is no measured upstream comparison.

## Known limits and blockers

- The current commands share a database file with exclusive process ownership. Offline imports and exports require a stopped listener. A future in-process scheduler should preserve consistent queries during sync.
- Structural RPSL import is strict about UTF-8, key format and duplicate identities. Real registry datasets and upstream fixtures still need differential testing.
- Public WHOIS implements only the documented command subset. Origin order is deterministic by configured source and then per-source lexical order. Upstream ordering and RPKI/route-preference behaviour require differential testing.
- Snapshot export can contain password hashes and requires explicit acknowledgement. The round trip exercises object recovery. Journal, serial and sync metadata recovery needs a versioned backup format.
- Replacing the synthetic 10,000-object database increased apparent file size from 4.75 MB to 135.28 MB. Profile page reuse and compaction with sustained replacements before making a storage-footprint claim.
- Docker Desktop reported an inaccessible `dockerInference` runtime endpoint. The screenshot and local reparse point match a reported Windows startup failure. Use the isolated Linux oracle workflow for interoperability and comparative benchmarks. [Docker issue](https://github.com/docker/desktop-feedback/issues/625)
- Local Windows GNU builds use Rust 1.99.1 and required adding Rust's self-contained tools and a checksum-verified MSYS2 assembler to the command's PATH. The repository does not depend on that workaround on a standard Linux or Windows MSVC development runner.

## Resume

1. Run formatting, Clippy and locked tests as shown in the [README](README.md). Run the RustSec audit and a Linux RPSL fuzz campaign.
2. Bring up the pinned upstream service on an isolated Linux runner using the [oracle instructions](tools/oracle/README.md). Load a licence-cleared corpus into both services and run the WHOIS differential harness. Record object counts, schema/format differences and exact wire evidence.
3. Extend the upstream oracle corpus for covering, covered, inverse and set queries. Add remaining RIPE flags and prove default source, error and ordering behavior with transcript comparisons.
4. Add versioned source journal events and a bounded in-process sync worker. Bind multi-step queries to one database view before enabling online updates. Preserve serial continuity and all-or-nothing index publication. Use pinned v4.5.4 fixtures before implementing broader NRTMv4 draft interoperability.
5. Advance GraphQL and authoritative submission on stable query and storage contracts. Keep all unmet security, interoperability and production readiness gates in the compatibility matrix.
