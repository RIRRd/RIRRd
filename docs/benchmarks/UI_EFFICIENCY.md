# UI and lookup efficiency — 3 November 2026

> Historical checkpoint: embedded RPKI and RTR were removed in beta.2 on
> 4 November 2026. Validator measurements and commands below apply only to the
> recorded historical source revisions. They are not current features. See
> [the IRR-only checkpoint](../compatibility/IRR_ONLY_BETA2_CHECKPOINT.md).

Branch: `feature/ui-efficiency`. Baseline: RIRRd `v0.1.0-beta.1`,
`a47609825e3db2f1428f8604e420a7605fd55949`. Measured candidate:
`20960d1649c44febeb925af2ea4df674df46c2c2`, from a clean checkout.
Raw results include source/driver SHA-256 values and individual paired timings.
These are bounded RIRRd-to-RIRRd optimization checks, separate from the historical
100,000-object RIRRd-to-official-IRRd comparison.

## RPKI lookup work

The immutable VRP table records the prefix lengths present in each address family.
Covering lookups skip lengths with no entries. A direct exact lookup remains first,
so common exact-valid announcements do not traverse that directory. AS0, maxLength,
any-authorizing-cover and unavailable/expiry semantics remain covered by tests.
An independent full-scan oracle checks both families, overlapping lengths and AS0.

The synthetic table has 1,048,637 prevalidated payloads: dense documentation/private
prefix blocks plus a broader set of lengths outside the queried ranges. It is
not a downloaded global ROA feed or a cryptographic validation workload. Each
profile uses 1,024 varied queries, 100,000 lookups per sample and three alternating
baseline/candidate rounds on the same pinned CPU. Both implementations must return
identical results. Thread CPU timing excludes scheduling preemption; wall timings
are also retained. The VM is shared with the live service.

| Profile | Baseline CPU µs/lookup | Candidate CPU µs/lookup | CPU-normalized lookup rate |
| --- | ---: | ---: | ---: |
| v4_exact_valid | 0.543 | 0.535 | 1.01× |
| v4_more_specific_valid | 1.863 | 1.886 | 0.99× |
| v4_invalid | 6.226 | 4.518 | 1.38× |
| v4_not_found | 4.727 | 3.456 | 1.37× |
| v6_exact_valid | 0.509 | 0.525 | 0.97× |
| v6_more_specific_64 | 3.760 | 2.286 | 1.64× |
| v6_more_specific_128 | 16.639 | 2.629 | 6.33× |
| v6_invalid | 14.487 | 6.779 | 2.14× |
| v6_not_found | 11.647 | 6.461 | 1.80× |

Exact-valid cases are approximately flat in this run (including a roughly 3%
IPv6 regression); this is not an across-the-board speedup claim. The larger gains
apply to the tested long-prefix, invalid and not-found paths. Real prefix-length
distributions, cryptographic refresh, RTR delivery and full request handling have
different costs. No whole-node throughput or minimum-host sizing is inferred.

[Raw ROV results](results/2026-11-03-rov-index.json).

## History delivery

The public history response is immutable between one-minute observations. It is
now serialized once and reused, with invalidation on the next recorded sample.
This cache contains aggregate telemetry only; registry queries and credentials
are not cached. The history file and privacy/retention bounds are unchanged.

The normal HTTP route was measured with 1,440 synthetic observations, four clients,
400 requests per sample and three alternating paired rounds. Each server runs in
its own process on the same selected CPU. Server CPU excludes the HTTP client.
Both implementations return the same schema/values; fixtures have separate creation
timestamps. Compact serialization changes whitespace and response length.

Baseline server CPU was **6.901 ms/request**; candidate **0.989 ms/request**,
a **85.7% reduction** in this workload. Peak server RSS in these runs
was 26.96 MiB versus 25.48 MiB.
These figures describe full-window history delivery, not whole-VM consumption or
IRR query speed. A newly started history with fewer observations has less encoding
work to save. No artificial history is deployed to the public service.

[Raw HTTP results](results/2026-11-03-history-gateway.json).

## UI and deployment

The project and community identities remain separate. Both now have rounded
panels/controls, calmer copy/typography and accessible mobile menus, including
documentation pages. The project example command is copyable. Project CSS is
9,085 bytes versus 11,295 bytes in the beta; the interactive chart code grows
slightly for responsive layout and inspection. No frontend framework, fonts from
a third party, analytics or runtime dependency was added.

Charts use their container width rather than scaling a wide fixed canvas into
small text on mobile. Unchanged input reuses the existing SVG. New data preserves
the inspected observation and keyboard focus; a Latest button resumes following.
Missing observations remain separate line segments. Only charts for the active
console page are rendered. Source publication receipts retain their open state
through refreshes. Browser tests exercise these actual interactions.

Static deployment now compares service/configuration files and restarts only
changed or stopped components. In the verified rollout, only the changed gateway
restarted: Caddy, the Rust registry and private submission service kept their PIDs.
The subsequent explicit native lookup/UI upgrade recovered nineteen-source health
in 1.79 s with the existing database generation; no database copy/reimport occurred.
Small prior configs, executable and release metadata are retained in
`/var/backups/rirrd-web/efficiency-native-20261103T193848Z`.

Installed native source: `1594d72724a66aa0cf9ca137b2cff2b17acf0f1f`.
SHA-256: `132aacee944da345f86f156cc25e6d886b198a07d83b978d744b350ffbc99eeb`.
Later benchmark/deployment/documentation commits do not change its compiled inputs.

## Verification

Native strict Clippy, format and locked all-target tests pass: 150 unit tests,
one existing ignored helper, 19 CLI/HTTP/TCP integration tests and two client tests.
The same 150/19/2 release tests pass under ARM64 QEMU. Fifteen gateway tests pass,
including immutable export reuse/invalidation and restart behavior. The RPKI fuzz
smoke completed 343,943 runs in 31 seconds without a crash. The locked dependency
audit has no known vulnerable crate and retains the disclosed unmaintained
rustls-pemfile warning.

Live Chromium checks cover both sites at 320/390/1440 px, mobile menus on both
homepages/docs, IPv4/IPv6/AS-set queries, source browsing, RPKI validation/batches,
SVG reuse, responsive redraw, retained selected sample/focus, missing-data gaps
and open source receipts. The independent loopback console is checked separately.
HTTPS/public RPKI checks use the normal exposed read-only entry points. Private
preview/fixture processes and benchmark databases are never deployed publicly.

External HTTPS probes also returned 200 with the new menu markup from Helsinki
(project IPv4), Montréal (project IPv6), São Paulo (community IPv4) and Newcastle
(community IPv6), resolving to the intended VM addresses. Globalping measurement
IDs are `2gM1Ww3GLWrMIS6tS00021RCp`, `2co47nlzG01ZhYJgM00021RCp`,
`2JOtpRGoHnuCB8Jj300021RCp` and `2mVIDDEKG1BAhLpcw00021RCp` respectively.
This verifies those observations, rather than universal reachability.

Commands to reproduce bounded comparisons:

```sh
python3 tools/bench/compare_rpki_index.py --baseline v0.1.0-beta.1 \
  --work /private/rov-bench --output /private/rov-result.json
python3 tools/bench/compare_history_gateway.py --baseline v0.1.0-beta.1 \
  --work /private/history-bench --output /private/history-result.json
python3 tools/check_project_sites.py --project https://rirrd.co.uk --community https://irrd.tools
python3 tools/check_chart_interactions.py --project https://rirrd.co.uk --community https://irrd.tools
python3 tools/check_community_ui.py --url https://irrd.tools
python3 tools/check_rpki_http.py --url https://irrd.tools
```

Logs/screenshots and generated corpus remain under `/tmp/rirrd-efficiency-*` and
`/var/lib/rirrd-dev/`. Measurements are local evidence; no new passing private CI
run is claimed. Beta status and all previously documented parity/qualification
gaps remain open. Main and the existing beta tag are not rewritten.
