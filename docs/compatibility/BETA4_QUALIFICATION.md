# Beta.4 qualification — 5 November 2026

The tested source is `024b48dae2c84d06ac18797d8558d50327c532e9`, built with
Rust 1.99.1 for Linux x86-64. The executable is 12,705,088 bytes, SHA-256
`25b2e9625a023a10ab88bdfa512e0ded1ac19789b7c78c2396bee1da174be564`.
The official oracle remains IRRd 4.5.4 at
`8e8080dab744a348b7d0e6efcb53f403301dd075`.
Full parity and production qualification remain incomplete.

## Functional and build evidence

| Check | Result and scope |
| --- | --- |
| Local core | 329 passed; two optional diagnostic cases skipped. Includes formatting, strict Clippy, locked Rust tests, external protocol contracts and HTTP load/recovery. |
| Bundled console | 16 real-browser cases passed. |
| Allocation/IRT | 157 shared schema decisions and 256 HTTP/query case groups passed against the pinned parser/daemon; normal CLI/HTTP rollback, references, source isolation and persistence also passed. |
| Existing authorization | 85 upstream comparison cases passed. |
| TCP/HTTP WHOIS | 48 TCP and 48 HTTP transcripts matched under the documented normalization. |
| Source/large-query corpora | 325 source-scope and 76 large-query assertions passed. |
| Aliases/indexed filters | 158 assertions passed, including seven explicitly documented behavior differences; these are not 158 identical upstream responses. |
| Other GraphQL | Existing organization/policy value and type checks, plus 15 inverse-filter comparisons and 11 argument-type checks passed. |
| Dependency audit | Zero vulnerabilities or warnings against freshly fetched RustSec commit `26fd2d43f7280a1fb925a3743ec463017afe7119`. |
| Publication hygiene | Candidate file audit and all-reachable-history Gitleaks scan passed. This does not guarantee detection of every possible secret. |

Passing CI for the tested source:
[Linux x86-64/ARM64, Windows, browser, coverage, audit and workflow validation](https://github.com/RIRRd/RIRRd/actions/runs/42060544507),
[official IRRd oracle](https://github.com/RIRRd/RIRRd/actions/runs/42060544455),
[BIRD/FRR and console integration](https://github.com/RIRRd/RIRRd/actions/runs/42060544521),
[container deployment](https://github.com/RIRRd/RIRRd/actions/runs/42060544464),
[fuzz smoke](https://github.com/RIRRd/RIRRd/actions/runs/42060544457), and
[source hygiene](https://github.com/RIRRd/RIRRd/actions/runs/42060544459).
Tagged publication separately requires the full release workflow and native
DEB/RPM install/systemd qualification; a candidate CI pass is not publication.

The local oracle was finally run against fresh isolated PostgreSQL/Redis state.
An earlier reused reference retained a protected-name tombstone from 3 November;
that setup-only failure is preserved in operator evidence. A new Docker bridge
was also unreachable under the host's existing firewall, so the clean reference
used fresh volumes on the already authorized isolated bridge. No firewall or
public registry changes were made for the oracle.

## Complete real-data answers

The immutable candidate passed all 39 complete-answer checks against the inactive
21-source, 19-class, **13,983,859-object** dataset. TCP, HTTP WHOIS and GraphQL
returned matching canonical prefix hashes, including **3,514,991 prefixes** in
one transit-set result. Counts, source cursors and configuration were unchanged.
Peak daemon RSS was **648.5 MiB**, under a three-CPU/3 GiB service ceiling and
64 MiB database cache. This was a warm-cache correctness sequence on a shared VM,
not an independent full-data IRRd comparison or minimum-host-memory test.
See [fixture scope and canonical-answer method](QUERY_SOURCE_PARITY.md).

## Deployed HTTPS regression

After a backed-up runtime-only dev upgrade, the same binary completed
**16,086/16,086 HTTP 200 requests** from 16:29:18–16:30:32 UTC. The driver offered
230 starts/s for 70 seconds, reached 229.8 starts/s, skipped 14 planned slots,
allowed at most 32 requests in flight and performed no retries. Every answer had
54 IPv6 prefixes in 909 bytes, SHA-256
`910eaff90f7475ad202573936f5ac4fa5820b352c9b63889be5d36198bdf52bb`.
There were no rejection, gateway, timeout, invalid-answer or transport failures;
recovery passed 5/5. RADB committed serial 6869287 at 16:29:39.201718 UTC;
RIPE and RIPE-NONAUTH also committed during the measurement.

Total request p50/p95/p99 were **17.68/22.98/26.04 ms**. Post-TLS waiting
p50/p95/p99 were **2.53/4.31/6.09 ms**. The test used new connections through
the real dev Caddy route, pinned IPv6, trusted-certificate/hostname validation,
and no DNS lookup. It originated on the hosting VM: **self-routed HTTPS evidence,
not WAN or maximum-capacity qualification**. No other deliberate test workload
or compiler was running; normal production and mirroring continued.

## One-CPU comparison with beta.3

The unchanged 100,000-object synthetic PERF fixture has SHA-256
`9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2`.
Compare beta.3 source `fa100c4509f76eccbf338fd69de753a2d834df23`, executable
`4269e11fd7dc0f0e20f742bab1c12bd48b8d22f7e3ca03e5cbc78f8204d405ca`,
with the candidate above. Each profile has one async worker, four blocking workers,
one CPU/100% quota and 128 MiB service limit. Four validating clients use separate
CPUs, reused connections and 50,000 ASN keys. Three ten-second repetitions follow
three-second warm-ups; profile order rotates. Filesystem caches may be warm.

| Metric | Beta.3, 64 MiB cache | Beta.4, 64 MiB | Beta.4, 16 MiB |
| --- | ---: | ---: | ---: |
| Median WHOIS queries/s | 9,562 | 9,698 | 8,978 |
| Worst per-run WHOIS p95, ms | 0.808 | 0.770 | 0.879 |
| Median GraphQL queries/s | 3,501 | 2,985 | 2,944 |
| Worst per-run GraphQL p95, ms | 2.443 | 2.698 | 2.793 |
| Median idle process PSS, MiB | 33.12 | 33.52 | 25.23 |
| Median fresh import, seconds | 7.86 | 8.49 | 8.90 |
| Database apparent size, MiB | 129.01 | 129.01 | 129.01 |

All **1,111,904 measured replies** validated without errors. WHOIS throughput
was similar; tiny GraphQL throughput was about **14.7% lower** at the same cache
budget. The new request snapshot currently adds a blocking-worker transition
before resolver work. Inspection identifies this as a possible contributor,
not a proven sole cause. `redb::begin_read()` can perform file I/O on a cold page;
moving it onto an async worker merely to improve warm-cache timing is unsafe.
A lazy shared snapshot inside the first resolver worker needs separate correctness,
cancellation, cold-cache and controlled performance qualification.

No general query-speed, import-speed or storage reduction is claimed. Allocated
database sizes were about 97.3 MiB in all profiles. The smaller cache lowers
process memory with a workload-dependent speed cost. The binary grew about 2.1%
with the additional features. The earlier verified full-catalog 28-to-20 GiB
repack remains the measured disk reduction; this beta changes no storage encoding.
Reproduce with `tools/bench/run_small_host.py`, the fixture generator and saved
binary/commit arguments described in [the benchmark guide](../../tools/bench/README.md).

## Small pinned-upstream smoke

A separate fresh **12-object** DIFFERENTIAL fixture was loaded identically into
RIRRd and official IRRd. Normalized exact payloads matched before measurement.
Three alternating runs used eight clients, one connection/request, one candidate
query worker and one official WHOIS worker. Both returned **3,000/3,000** valid
measured replies. Median throughput was **1,327 vs 97 queries/s**; median per-run
p95 was **9.35 vs 97.49 ms**. Candidate peak RSS was 10.03 MiB and allocated data
1.57 MiB. This tiny warm-cache connection-per-query smoke is not full-dataset
capacity, sustained load or a comparable whole-stack memory ratio. Reproduce with
`tools/oracle/benchmark_pair.py` and the pinned isolated reference configuration.

Raw logs, databases, source data, per-request timings and operator deployment
receipts remain outside Git. Public fixtures are synthetic. No test update was
sent to a public registry and no live router was changed.
