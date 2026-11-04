# Continuous verification checkpoint — 4 November 2026

Tested suite revision: `3627e1073bf690e268eb198ef337dbf67c9399b8` on
`feature/continuous-verification`. The [testing guide](../TESTING.md) maps test
areas, cadence, commands, artifacts and remaining coverage limits. The public
recorded results are at <https://rirrd.co.uk/testing/>. Later evidence/static
snapshot commits are distinguished from that exact clean tested revision.

## Executed local gates

| Gate | Result |
| --- | --- |
| Formatting and strict locked Clippy | Passed |
| Rust tests | 151 unit + 19 integration + 2 client tests passed; 1 existing subprocess helper ignored as a standalone test |
| Independent TCP/HTTP/GraphQL and recovery contracts | 56 passed |
| Report/provenance/coverage integrity | 15 passed |
| Public gateway isolation | 15 passed |
| Private community service | 4 passed |
| Isolated website/browser workflows | 14 passed, including both sites, real synthetic-registry queries, source browsing, mobile views, accessible field names, manifest/served-asset integrity and private fixture submissions |
| Core + browser case totals | **276 passed, 0 failed, 1 skipped**; gates without test cases are excluded from these counts |
| Source coverage | **93.11%, 13,032 / 13,996 source lines**, enforced floor **92%** |
| ARM64 QEMU independent contracts | 56 passed against the existing ARM64 release binary with the same unchanged Rust protocol implementation |
| RPSL / NRTMv3 / RPKI fuzz smoke | 370,149 / 190,448 / 150,825 executions, 16 seconds each; no crash |
| Workflow lint | All workflows passed pinned Actionlint 1.7.12 in a read-only, network-disabled container |
| Dependency audit | Current fetched advisory DB; 341 locked dependencies, no known vulnerable dependency. Existing unmaintained rustls-pemfile / RUSTSEC-2025-0134 warning remains disclosed |

Rust source coverage includes test modules within `src`. External fixture/test
harness and dependency files do not enter the summary. It records executed
lines, not correctness, complete cryptographic coverage or full feature parity.
The 56 external cases are also executed against the instrumented binary. JSON,
JUnit, logs, LCOV and HTML are retained privately outside Git. The public JSON
contains counts, source paths/line summaries and exact source provenance;
it excludes failure traces, runtime paths, visitor data and credentials.

The standalone ignored helper is
`storage::tests::crash_replacement_child`; its parent test invokes it as a
subprocess to verify crash recovery. It is not counted as a passed standalone
case. Meta-tests exercise intentional failure conditions in nested fixtures;
those errors must appear as failures in exported reports.

## Official pinned oracle and container checks

The cached official IRRd image was verified as v4.5.4, corresponding to the
existing pinned wheel/source baseline
`8e8080dab744a348b7d0e6efcb53f403301dd075`. A fresh isolated PostgreSQL/Redis
project exercised the existing workflow against synthetic fixture sources:

- 45/45 TCP and 45/45 HTTP WHOIS cases matched.
- 71 HTTP authoritative authorization decisions matched both implementations.
- 10 organisation, 25 policy/router and 16 AS-block/domain schema cases passed.
- Five NRTMv3 omission/framing and three unsafe-member cases retained the
  documented stricter RIRRd behavior.
- Organisation GraphQL values/nine field types and four policy/router value
  cases/32 field types matched.

These results exercise the existing bounded corpus; they do not establish
bidirectional NRTMv4 interoperability, complete OpenPGP/account workflows or
full schema/policy parity.

The distribution-based Docker image built from the current source. A separate
private Compose project imported a synthetic snapshot, became healthy, returned
an exact byte-framed TCP response, shut down and passed persisted database
verification. New volumes/containers were removed after both suites.

This VM intentionally permits forwarding on its existing `irrd-oracle0` test
bridge and blocks new unapproved bridges. An initial project on a new bridge
could not become healthy. Reusing the existing isolated network fixed the
oracle without altering firewall rules or production services. Docker's default
build network also could not resolve external dependencies; this local build
used `--network=host`. Standard GitHub runner smoke configuration remains intact.
The advisory DB was refreshed using a per-command GitHub address override after
the default network path failed; the freshly updated checkout was then audited.

## Build automation and publication

Core CI now exports JSON, individual JUnit outcomes, gate logs and a step summary
on each push/PR/manual run for Linux x86-64, Linux ARM64 and Windows. Independent
browser and source coverage jobs, the dependency audit and workflow validation
run as separate gates. Short three-target fuzz runs and container smoke checks
also run on every push/PR. Dedicated upstream/router/native-package workflows
retain their existing triggers, and longer fuzzing stays scheduled.

The runner clears stale results before reruns, rejects missing/invalid case
records and fails for command errors, setup errors, subtest errors and blocked
required gates. Deliberately failing exporter/coverage/publication fixtures
verify that a green process cannot conceal those failures. Failed commands do
not prevent unrelated safe gates from producing diagnostic results.

The source and public snapshot are pushed on a feature branch. Main and the
beta tag are unchanged. This session has a repository SSH deploy key but no
GitHub API authentication (`gh auth status` is unauthenticated; anonymous repo
API returns 404). New private Actions runs therefore cannot be confirmed here.
The website labels the verified snapshot **local**, links the exact tested
revision and workflow, and does not invent passing remote runs. CI artifacts
include actual run URLs so authenticated CI results can be published later.

Only static site files are deployed for this continuation. The public native
registry binary, source generation, mirror workers and listeners are unchanged.
All test registry/queue/browser/Compose processes are cleaned up. Full parity
and production qualification remain open in the compatibility matrix.
