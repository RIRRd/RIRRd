# Compact ROV index and direct search — 3 November 2026

Deployed source: `eed64b8a762a98a3dade72bd5dcb21eaa78c5eae` on
`feature/compact-rov-search`. Previous optimized baseline:
`fc7bef86aed6e3c97451dc0abed567d482638788`. This continues the
[previous efficiency work](UI_EFFICIENCY.md). These are resource/usability
improvements; full IRRd parity and production qualification remain incomplete.

## Allocation and CPU evidence

The ROV lookup table now stores IPv4 and IPv6 records separately, avoids a
wide intermediate record array during JSON parsing, and returns substantial
capacity left after deduplication. Binary search compares the network before
inspecting length/origin. Validation still considers every covering payload,
including default routes, different max lengths, multiple origins and AS0.
Explanations retain their 32-record bound, independently of the verdict.

[Raw paired results](results/2026-11-03-compact-rov-index.json) use 1,048,637
synthetic payloads, approximately equal IPv4/IPv6 counts, nine lookup profiles,
100,000 requests per profile per round and three alternating paired rounds.
The checksums and every preflight verdict match the baseline. The same Linux
VM has 24 available x86-64 vCPUs (Xeon E5-2683 v4), approximately 62 GiB RAM,
and Debian kernel 6.12.115+deb13.1-amd64. The kernel test is pinned to vCPU 23.
No import or benchmark request is sent to public registries.

Owned index allocation, including Vec capacity and directories, falls from
**50,334,744 bytes (48.00 MiB) to 23,070,316 bytes (22.00 MiB), 54.17% less**.
This excludes allocator metadata, temporary fixture construction, cryptographic
validation, RTR history and all other process memory. It is not an RSS saving
claim or minimum-host specification. The one recorded construction took 0.292 s
versus 0.037 s; this single observation is not a repeated import benchmark.

| Profile | Baseline CPU µs/query | Candidate CPU µs/query | Relative lookup rate |
| --- | ---: | ---: | ---: |
| v4_exact_valid | 0.538 | 0.374 | 1.44× |
| v4_more_specific_valid | 2.028 | 1.102 | 1.84× |
| v4_invalid | 4.093 | 1.633 | 2.51× |
| v4_not_found | 3.286 | 0.912 | 3.60× |
| v6_exact_valid | 0.519 | 0.469 | 1.11× |
| v6_more_specific_64 | 2.378 | 1.611 | 1.48× |
| v6_more_specific_128 | 2.669 | 1.647 | 1.62× |
| v6_invalid | 5.697 | 2.750 | 2.07× |
| v6_not_found | 4.423 | 1.897 | 2.33× |

Rates include parsing the query prefix. They use aggregate thread CPU time,
with wall time retained in the raw file. They are synthetic lookup kernels,
not HTTP/TCP throughput, complete-validator performance or an IRRd comparison.
The final version improves every measured profile, 1.11–3.60×. The public
historical IRRd comparison remains separately labelled at its recorded revisions.

`GET /v1/rpki/status` now reports `rov_index_bytes`. The RPKI console explains
that it estimates the lookup index's owned capacity and is part of process
memory. An unavailable dataset reports null instead of an invented measurement.

## Search and presentation

The homepage submits directly to a single bounded query. Examples can explicitly
request the same action. The run flag is consumed once; reopening a prepared
URL fills the form without silently repeating a query. Searches default to all
configured sources; operators can select one explicitly for reproducible output.
Metadata loads first. Query/API/router pages defer live status, history and RPKI
requests until those views are needed. The homepage IPv6 handoff is checked to
make exactly one WHOIS request and zero initial monitoring requests.

Overlapping cancelled responses cannot replace a newer result or re-enable its
button. Result formatting uses the submitted query type even if the form changes
while the request is pending. Browser fixtures exercise both races. Unknown
section links correctly render and refresh the overview.

The project website uses a simpler feature list instead of repeated feature
cards. The community search has a clear node label and example controls. Active
query choices are visible and exposed with aria-pressed. Mobile counters stay
on one line at 320/390 px; Chromium checks text ranges as well as page overflow.
The two brands, community directory and independent console remain separate.

## Verification and deployment

Strict Clippy, formatting and locked all-target Linux tests pass: 151 unit tests,
one existing ignored helper, 19 integration tests and two client tests. The same
151/19/2 release tests pass under ARM64 QEMU for the compact-index/search code;
subsequent mobile-style and unknown-section changes are checked in Chromium.
The final RPKI fuzz smoke completed **334,509 runs in 31 seconds** without a
crash. The dependency audit reports no known vulnerable locked dependency and
retains the previously disclosed unmaintained rustls-pemfile advisory warning.
No new dependency or registry storage format is introduced.

Browser checks cover both sites at 320/390/1440 px, direct IPv4/IPv6/AS-set/ASN
queries, presets, delayed/cancelled responses, source browsing, mobile navigation,
RPKI checks/batches, chart inspection/resizing/gaps and retained source receipts.
A private preview exercised the submission form with a separate private queue;
no test operator contact was submitted to production. The final independent
self-hosted binary passes its own normal HTTP console checks.

The native binary was backed up to
`/var/backups/rirrd-web/compact-native-20261103T200106Z` before replacement.
The registry query inventory recovered in 4.43 seconds, with all 19 sources and
the existing generation preserved. No database copy, migration or reimport was
performed. Fresh RPKI validation completed in 46.25 seconds after restart; the
public API then reported 1,029,706 payloads (792,646 IPv4 / 237,060 IPv6), a
17,900,616-byte index (17.07 MiB), all five anchors and 98 disclosed rejected
publication points. Origin/batch checks and malformed/oversized failures passed
through the normal HTTPS API. Caddy, gateway and submission-service PIDs were unchanged through
both the native update and the static-site deployment. Caddy configuration
validation passed before deploying the two site releases. Static roots remain
`/var/www/rirrd-project` and `/var/www/irrd-tools`; native service is `rirrd-live`
and binary `/usr/local/libexec/rirrd-live`.

Both domains' public A/AAAA records resolve through Google DNS to 192.67.35.6
and 2a0f:6280:1d:1940:e6d8:d666:4268:d4c8. No DNS change is required. The web
server retains trusted automatic TLS, HTTP-to-HTTPS redirects and existing SSH
access. No registry write, mirroring-control or RTR listener was made public.

Public Chromium checks passed after rollout. IPv4 and IPv6 HTTPS return 200
with certificate verification, and HTTP returns 308 to each domain's HTTPS URL.
External probes also returned 200, trusted TLS and the new page content:

| Site / family | Probe | Globalping measurement |
| --- | --- | --- |
| rirrd.co.uk IPv4 | Falkenstein | `2qRmCAMQtUS6zdA7300021RD8` |
| rirrd.co.uk IPv6 | Lagos | `26DBBgzjpNJQCrVsU00021RD7` |
| irrd.tools IPv4 | Falkenstein | `2KmxTuw7xenpkyQAa00021RD7` |
| irrd.tools IPv6 | Spokane | `2zsMJmj96jjHJzjav00021RD7` |

An earlier Lagos IPv4 probe timed out establishing TCP; the subsequent European
IPv4 observation passed. These are specific observations, not universal
reachability guarantees. Full results remain outside Git. Publication filename/
blob checks and redacted Gitleaks scans cover reachable history and the candidate
source tree. Private preview servers, fixture service and benchmark processes
were stopped after verification. Production services remain active.

Commands to reproduce:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo audit --db /private/advisory-db --no-fetch
python3 tools/bench/compare_rpki_index.py --baseline fc7bef8 \
  --work /private/compact-index --output /private/compact-index.json
python3 tools/check_project_sites.py --project https://rirrd.co.uk --community https://irrd.tools
python3 tools/check_community_ui.py --url https://irrd.tools
python3 tools/check_chart_interactions.py --project https://rirrd.co.uk --community https://irrd.tools
python3 tools/check_rpki_http.py --url https://irrd.tools
```

Generated fixtures, screenshots, full logs, caches and private deployment state
remain outside Git under `/var/lib/rirrd-dev/`, `/var/backups/` and `/tmp/`.
Local verification is not a new passing private CI run. Authentication/account,
remaining class coverage, mirroring, live policy/query parity and release gates
remain open. Main and the existing beta tag are unchanged.
