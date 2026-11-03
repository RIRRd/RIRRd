## UI and resource-efficiency continuation — 3 November 2026

The [efficiency checkpoint](docs/benchmarks/UI_EFFICIENCY.md) records deployed
rounded site/console layouts, preserved chart inspection, responsive SVG rendering,
immutable history export caching and ROV prefix-length indexes. Raw paired
measurements, normal-entry browser/API checks, Linux/ARM QEMU tests and RPKI fuzz
results are linked. These bounded improvements do not close full parity or release
qualification gaps.

## Two-domain beta launch — 3 November 2026

The [beta checkpoint](docs/compatibility/BETA_LAUNCH_CHECKPOINT.md) records the
project showcase on rirrd.co.uk, the separate community network on irrd.tools,
new identities/graphs, typed RPKI refresh indexing, covering ROAs, bounded batch
validation, beta packages/notices and Linux/ARM/oracle/fuzz evidence. Public GitHub
visibility and release/CI verification still require repository/admin authentication.
Full parity and production qualification remain incomplete.

## Community website and independent console — 3 November 2026

The [UI and operations checkpoint](docs/compatibility/COMMUNITY_UX_CHECKPOINT.md)
records the deployed community website, separate independent console, private
reviewed node submissions, visible IPv4/IPv6/AS-set/ASN and BIRD/FRR examples,
responsive browser checks and reduced telemetry polling. Nineteen live sources
remain available. Full parity remains incomplete; this UI checkpoint does not
close authentication, mirroring, policy or release qualification gaps.

## UK node, nineteen public sources and embedded RPKI — 3 November 2026

The [current checkpoint](docs/compatibility/UK_NODE_PUBLIC_SOURCES_RPKI.md) records
nineteen loaded public sources, about 4.99 million RPSL objects, seven incremental
feeds, twelve snapshot feeds, public ARIN query support, live class/key browsing,
API/router commands and in-process Routinator-based RPKI validation/RTR. Earlier
owner-enrollment assumptions are superseded. Full parity remains incomplete.

## Continuous mirroring continuation — 3 November 2026

See [the checkpoint](docs/compatibility/CONTINUOUS_MIRROR_CHECKPOINT.md) for the
source bootstrap/serial handoff, filtered NRTMv3 profile, quiet-source cursor
persistence, live query/schema console and remaining source-specific work.
Snapshot age, poll success and telemetry refresh are now separate concepts.
Full parity and production qualification remain incomplete.

## Expanded sources and write classes — 3 November 2026

The public service now serves 2,610,758 routing objects across twelve sources.
RADB, ALTDB, BELL, REACH, BBOI, TC and CANARIE were added to the initial five.
A failed TC import preserved the live generation; the resumed candidate verified
its completed imports and activated only after complete integrity/count checks.
Forty-six unsafe member records are withheld and disclosed, rather than repaired
into invented names. Five feeds retain explicit access/connectivity blockers.

Authoritative AS-block and domain updates increase the bounded write profile to
15 classes. Same-source maintainer/contact checks, zone-contact delete protection,
rollback and redaction pass normal CLI/HTTP/TCP tests. Pinned comparison passes
71 HTTP decisions, 45 TCP and 45 HTTP WHOIS cases, and 16 new schema cases.
Native Linux and ARM64 QEMU release tests pass 139 unit/17 integration/two client
tests. The latest fuzz smoke completed 177,454 executions in 31 seconds.
See [the expanded checkpoint](docs/compatibility/EXPANDED_SOURCES_CHECKPOINT.md).
Full parity remains incomplete; current release/CI/access limits still apply.

## Live public read-only continuation — 3 November 2026

The owner authorized a live experimental registry after the static-site checkpoint.
WHOIS/bgpq4 now uses `rirrd.co.uk:43`; the public HTTPS dashboard queries this
RIRRd database and reports measured telemetry. Five reviewed routing feeds contain
1,080,669 objects. ARIN remains TCP-only under its distribution terms. Twelve
other tracked feeds have explicit pending permission/transport actions rather
than invented coverage. See the [live checkpoint](docs/compatibility/LIVE_IRR_CHECKPOINT.md)
and [deployment guide](docs/operations/LIVE_SERVICE.md).

Published large policies exposed the snapshot parser's 64 KiB bound. Snapshot
admission/index extraction now accepts bounded 4 MiB policies while submission
admission stays 64 KiB. The mirror preparation controller preserves Unicode
paragraph separators and resolves duplicate canonical identities using the last
record, matching pinned upstream's snapshot upsert behavior. Duplicate/malformed
lookup counts are disclosed. The normal CLI still rejects duplicate imports.

Latest local native and ARM64-QEMU release checks pass 138 unit/16 integration/two
client tests; formatting, strict Clippy, 227-crate RustSec audit and a 221,757-run
fuzz smoke pass. The pinned 39 TCP/39 HTTP WHOIS, 47 authoritative decisions and
GraphQL fixtures still pass. Live browser and real BIRD/FRR candidate checks pass.
Publication tree/history scans found no secrets under the reviewed default rules.
The repository remains private, main unchanged, and new Actions evidence unavailable.
Full parity and production qualification remain incomplete.

## Dashboard, small-host and publication continuation — 3 November 2026

MIT licensing and explicit AI disclosure/contribution guidance are implemented. A bundled
read-only console, configurable redb cache/runtime budgets, and real bgpq4 interoperability
with BIRD/FRR syntax checks are implemented. See the [small-host checkpoint](docs/benchmarks/SMALL_HOST_CHECKPOINT.md)
for nine bounded imports, 1,500,091 validated responses and a separate cache-eviction
recheck. Import RSS improves at a measured import/query trade-off; this does not establish
full parity or a minimum whole-VM requirement. Current Linux checks pass 137 unit,
15 integration and two client tests, with the same release tests under ARM64 QEMU.
The 39 TCP / 39 HTTP WHOIS and 47 authoritative HTTP pinned-oracle cases still pass.
New CI workflows are present; authenticated Actions results remain unavailable.

The user subsequently authorized a live public **test** registry, dashboard and router
queries. Work is extending the private/synthetic console to real read-only routing
sources with documented permissions, measured status and isolated refresh jobs.
Submissions/administration stay private. Full parity and release qualification remain open.

# Autonomous development checkpoint

## GraphQL and scale comparison continuation

The [100k synthetic checkpoint](docs/benchmarks/LINUX_SYNTHETIC_100K.md) records
normal-entry-point comparisons, the validating native load client and the public
site's measurement methodology. Policy/router GraphQL values and 32 field types
across four classes now match the pinned baseline for the selected fixtures.
Lazy resolver attributes remove eager work for identity-only queries. The
before/after timing check is reported even when its run ranges overlap.
Full parity and production qualification remain incomplete.

## Upgraded VM continuation

The [24-CPU checkpoint](docs/compatibility/UPGRADED_VM_CHECKPOINT.md) adds four
authoritative class profiles, bounded maintainer hash replacement, 47 live HTTP
authorisation comparisons, ARM64 release tests under QEMU, and repeated timing
samples. Full parity remains open; native CI and full release/security gates
remain separate from these local results. The public comparison remains a static
site on codex/coming-soon and the source repository remains private.

## Linux VM continuation

See [Linux VM checkpoint](docs/compatibility/LINUX_VM_CHECKPOINT.md) for the
organisation implementation, expanded oracle and local Linux evidence. Full
parity remains open. New CI run verification needs authenticated Actions access.
The public coming-soon site is deployed independently from `codex/coming-soon`.

## Baseline

IRRd v4.5.4, commit `8e8080dab744a348b7d0e6efcb53f403301dd075`, is the pinned compatibility oracle. The [feature matrix](docs/compatibility/IRRD_FEATURE_MATRIX.md) records the reference behaviour, requirements and evidence for each area. [Architecture decisions](docs/ARCHITECTURE.md) record the initial storage selection and threat model. [Benchmark methodology](docs/benchmarks/METHODOLOGY.md) defines the controls for comparative testing.

## Implementation

| Milestone | Checkpoint |
|---|---|
| 0 Research | Baseline, feature and RPSL schema inventory, standards links, threat model and benchmark design recorded. Storage trade-offs are qualitative pending measured evaluation and the inventory needs differential evidence. |
| 1 Foundation | CLI, validated TOML, structured JSON logs, process lifecycle, embedded store and Windows/Linux CI workflow. CI execution and audit results are separate verification gates. |
| 2 RPSL and storage | Structural parsing for 19 classes, canonical identities, bounded plain/gzip snapshot replacement, exact and origin indexes, and logical verification. Prefix, set and reverse indexes are implemented. Full attribute and reference validation remains open. |
| 3 Queries | Persistent TCP, source/exclusion state, exact/covering/covered prefixes, origin and inverse indexes, and direct and recursive set expansion. Loopback HTTP serves WHOIS and bounded GraphQL object, prefix, set and status queries. Broader RIPE flag combinations, full GraphQL schema and upstream interoperability remain open. |
| 4 Mirroring | Local snapshot import/export and tested object round trip. NRTMv3 applies finite contiguous transcripts and persists the serial and retained history atomically. Configured peers can retrieve bounded, credential-redacted ranges over TCP. Offline NRTMv4 verifies and atomically applies signed snapshots and delta chains with persisted replay, session and key state. Authoritative NRTMv3 publication uses a separate local serial, and NRTMv4 has bounded one-shot HTTPS retrieval. NRTMv4 publication and scheduling, filtered-gap NRTMv3 and full live mirror interoperability remain open. |
| 5–7 Registry services | Offline origin validation evaluates validator-produced ROA feeds. CLI and opt-in loopback HTTP support atomic contact, route, set, aut-num and existing maintainer changes. They enforce bcrypt proof, references, protected names, same-source route parent authorization and atomic audit events. Full object-class validation, additional authentication schemes, live RPKI/scope filtering and administration remain open. |
| 8–10 Release | Initial local tests, RustSec scan, RPSL fuzz target, process-kill recovery test and synthetic smoke benchmark. Linux fuzz smoke and native Debian and RPM package tests passed. Host-crash and storage-failure recovery, broader security review, production migration and signing remain open. |

## Verification

- The authoritative ROA and scope admission checkpoint passed 108 unit and nine end-to-end tests, strict Clippy, and a 207-dependency RustSec scan. HTTP tests reject a route beyond the ROA maximum length and a covering route that overlaps an excluded network.
- The live Linux oracle matched all 25 TCP WHOIS and 25 HTTP WHOIS cases against pinned IRRd 4.5.4. [Oracle run](https://github.com/Peravix/Rirrd/actions/runs/41616928804)
- Native Debian and RPM packages passed install, bootstrap, WHOIS and post-SIGTERM verification on x86-64 and ARM64. Debian also passed systemd-supervised startup, WHOIS and shutdown on both architectures. Release signing remains open. [Package run](https://github.com/Peravix/Rirrd/actions/runs/41616928832)
- A Linux libFuzzer smoke run completed 2,460,454 executions in 61 seconds. Long fuzz campaigns remain open. [Fuzz run](https://github.com/Peravix/Rirrd/actions/runs/41613974874)
- Opt-in legacy `MD5-PW` supports migration of existing maintainers to bcrypt. Independent OpenSSL vectors checked the verifier; the CLI test covers default denial, source opt-in and rotation. New legacy credentials remain rejected.
- Scheduled NRTMv3 sync, additional RIPE modifiers and GraphQL root-source pinning passed 101 unit tests and eight end-to-end tests on Windows. The real TCP mirror test retries a malformed response and publishes the next valid update. Debian/RPM packaging definitions were linted and are queued for native Linux runtime checks.
- The NRTMv4 apply checkpoint passed 96 unit tests and seven end-to-end tests on Windows. A signed but malformed delta rolled back data and version, and a later valid delta advanced both. Signed-fixture tests cover key rotation and session rollover. The Linux oracle passed 13 exact-key WHOIS cases against pinned upstream 4.5.4. The 25-case expanded corpus has a local self-check and is queued for live comparison.
- The authoritative route checkpoint passed 89 unit tests and seven end-to-end tests on Windows, plus strict Clippy. Linux x86-64 and ARM64 passed the earlier CLI authoritative checkpoint. Foundation Docker Compose runtime passed in GitHub Actions.

- Query checkpoint on Windows passed 36 unit and 3 end-to-end tests, including per-IP admission, TCP pipelining and session isolation, prefix boundaries, inverse lookups, set expansion and credential-redacted GraphQL HTTP. Root dependency audit passed with 160 locked dependencies. An upstream-oracle job is configured for a Linux runner.
- NRTMv3 checkpoint on Windows passed 47 unit tests and four end-to-end tests. The new TCP/file test exercises rollback on a bad trailer, replay rejection, serial persistence and deletion via a loopback peer. Transport tests cover absolute deadlines and no-update replies.
- The offline ROV and GraphQL prefix checkpoint passed 53 unit tests and five end-to-end tests on Windows. It covers all three ROV states, dual-stack and AS0, GraphQL ASN/AS-set prefix queries, and a TCP per-IP exemption that preserves the global cap.
- The NRTMv3 server checkpoint passed 56 unit tests and five end-to-end tests on Windows. One Rust registry replayed the retained journal from a second Rust registry and then received a clean no-update reply. The test also checks source ACL denial and credential redaction.
- The NRTMv4 verification and compaction checkpoint passed 66 unit tests and six end-to-end tests on Windows. A separately generated OpenSSL P-256 signature also verified through the CLI. The root lockfile audit passed with 200 dependencies after upgrading the vulnerable crypto-bigint dependency to 0.7.10 through p256 0.14.0.
- Non-root Docker/Compose and systemd deployment examples are in [deploy](deploy/README.md). Compose configuration, daemon configuration and workflow shell syntax passed local validation. Linux x86-64 and ARM64 CI and container smoke definitions are present. Image build, runtime container and systemd startup, native packages and release signing remain open.
- The [upstream issue inventory](docs/compatibility/IRRD_ISSUE_BACKLOG.md) tracks 34 open issues and six open pull requests. Added local coverage for parts of #349 and #608, and an explicit offline source purge for part of #952.
- Windows Rust 1.99.1 passed formatting, Clippy with warnings denied, 19 unit tests and one CLI/TCP integration test. One ignored helper is invoked by the process-kill test, which confirmed the last acknowledged snapshot and indexes survived termination during replacement.
- The RustSec 0.22.2 scan checked 83 dependencies in the root lockfile against 1,402 advisories and exited successfully. The released Windows executable was checksum-verified. The source build of the audit tool required a C compiler absent from this host.
- The exact-key differential harness passed 13 fixture cases in a Rust-to-Rust self-check. This exercises the harness. Upstream execution remains pending. The [Compose oracle](tools/oracle/README.md) passed configuration validation only.
- The isolated RPSL fuzz target type-checked. A scheduled Linux CI job is configured for libFuzzer. No full fuzz run or remote CI run has been observed.
- The [Windows synthetic smoke report](docs/benchmarks/WINDOWS_SMOKE.md) records three release-build repetitions and the raw results. At eight concurrent clients the mean was 10,903 queries/s with p95 0.88–1.14 ms. There is no measured upstream comparison.

## Known limits and blockers

- The current commands share a database file with exclusive process ownership. Offline imports and exports require a stopped listener. The in-process NRTMv3 scheduler applies updates atomically. NRTMv4 scheduling remains open.
- Structural RPSL import is strict about UTF-8, key format and duplicate identities. Real registry datasets and upstream fixtures still need differential testing.
- Public WHOIS implements only the documented command subset. Origin order is deterministic by configured source and then per-source lexical order. Upstream ordering and RPKI/route-preference behaviour require differential testing.
- Snapshot export can contain password hashes and requires explicit acknowledgement. The round trip exercises object recovery. The full database backup format preserves journals, serials, sync trust and protected names. Keep these archives in a private directory.
- Replacing the synthetic 10,000-object database increased apparent file size from 4.75 MB to 135.28 MB. Offline compaction reduced it to 11.84 MB in one run and reopened with all 10,000 objects. Profile page reuse during sustained replacements to address peak storage.
- Docker Desktop reported an inaccessible `dockerInference` runtime endpoint. The screenshot and local reparse point match a reported Windows startup failure. Use the isolated Linux oracle workflow for interoperability and comparative benchmarks. [Docker issue](https://github.com/docker/desktop-feedback/issues/625)
- Local Windows GNU builds use Rust 1.99.1 and required adding Rust's self-contained tools and a checksum-verified MSYS2 assembler to the command's PATH. The repository does not depend on that workaround on a standard Linux or Windows MSVC development runner.

## Resume

1. Run formatting, Clippy and locked tests as shown in the [README](README.md). Run the RustSec audit and a Linux RPSL fuzz campaign.
2. Bring up the pinned upstream service on an isolated Linux runner using the [oracle instructions](tools/oracle/README.md). Load a licence-cleared corpus into both services and run the WHOIS differential harness. Record object counts, schema/format differences and exact wire evidence.
3. Extend the upstream oracle corpus for covering, covered, inverse and set queries. Add remaining RIPE flags and prove default source, error and ordering behavior with transcript comparisons.
4. Add authenticated recovery controls, NRTMv4 server publication and a scheduler around the verified HTTPS application path. Preserve source-scoped atomicity and extend the pinned oracle to signed mirror interoperability before broadening the draft profile.
5. Advance GraphQL and authoritative submission on stable query and storage contracts. Keep all unmet security, interoperability and production readiness gates in the compatibility matrix.
