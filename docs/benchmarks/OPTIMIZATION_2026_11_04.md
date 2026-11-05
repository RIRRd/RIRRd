# Allocation and release-build checkpoint — 4 November 2026

Runtime changes: `b1cbc8d2439b94317df2a9df292e7dc0ffb0d399` and
`d1d91cacdd06fdb205c984f3ab0278ef19fab2fd`. Benchmark guard correction:
`83f7931b5c7574af6a2bb853a8320a740af1abb7`; its Rust and Cargo inputs are
identical to the measured runtime. Baseline:
`e16b0ce4103a33ce010b3a33522ce125640b348e`.

[Raw results, artifact hashes and CI evidence](results/optimization-2026-11-04.json)
contain every completed measured repetition. Partial/failed harness attempts are
excluded. Fixtures, databases, caches and host deployment files remain outside Git.

## Changes

- Snapshot import transfers its owned record buffer into the parser, reads lines
  directly into that buffer and avoids allocating lowercase attribute names when
  the input is already lowercase. Validation, original text and byte limits remain.
- WHOIS rendering avoids intermediate line arrays and per-record copies for
  ordinary LF records. CRLF, UTF-8 byte framing, result limits and the shared
  credential filter retain their existing behavior.
- Single-source origin queries use the index's existing uniqueness guarantee,
  avoiding a second set of prefix strings. Multi-source queries retain source
  priority and deduplication, without allocating strings for repeated prefixes.
- Portable release builds use thin LTO, one codegen unit and debug-section
  stripping. CPU target, unwind behavior, database format, durability and all
  protocol/admission limits remain unchanged. Optimized builds take longer.

## One-CPU measurements

Debian 13, Linux x86-64, 64-vCPU/62-GiB VM. Server/import units used CPU 0 and a
100% CPU quota; the four-client validating driver used CPUs 16–19. Import units
had 256 MiB limits, query units 128 MiB. Other public services stayed active.
No compiler, fuzzer or upstream oracle ran during timing. These are constrained
process measurements on a larger VM, not a minimum-host qualification.

Both baseline and candidate 64 MiB profiles use one async worker and four
blocking workers. The 16 MiB candidate changes only the database cache budget.
The original 100,000-object synthetic PERF fixture is 15,485,046 bytes, SHA-256
`9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2`.

Three fresh imports per profile, rotating order, followed by three 10-second
WHOIS/GraphQL runs per profile with three-second warmups and reused connections.
Every import is checked through `database verify`. Every measured reply is
validated for framing and identity/source; GraphQL also checks class and errors.
Input and database file caches can be warm.

| Measurement | Previous, 64 MiB cache | Optimized, 64 MiB cache | Optimized, 16 MiB cache |
| --- | ---: | ---: | ---: |
| Executable, MiB | 14.79 | 11.60 | 11.60 |
| Gzipped executable, MiB | 4.88 | 4.36 | 4.36 |
| WHOIS median queries/s | 11,942 | 12,426 | 11,175 |
| WHOIS worst per-run p95, ms | 0.704 | 0.614 | 0.697 |
| WHOIS worst per-run p99, ms | 0.939 | 0.829 | 0.961 |
| GraphQL median queries/s | 3,526 | 4,164 | 3,764 |
| GraphQL worst per-run p95, ms | 2.572 | 1.936 | 2.096 |
| GraphQL worst per-run p99, ms | 3.409 | 2.598 | 2.840 |
| Idle process PSS, median MiB | 33.55 | 32.89 | 24.64 |
| Import process peak RSS, median MiB | 18.34 | 18.32 | 13.64 |
| Import wall time, median seconds | 7.03 | 8.29 | 8.88 |
| Import CPU time, median seconds | 6.13 | 5.83 | 6.13 |
| Database apparent size, MiB | 129.01 | 129.01 | 129.01 |

At the same cache budget the executable is 21.6% smaller, median GraphQL
throughput rises 18.1%, WHOIS throughput rises 4.1%, and worst per-run p95 falls
24.7% and 12.9% respectively. Idle PSS falls about 2%. The lower-cache profile
uses less memory with lower WHOIS throughput. It is an explicit sizing trade-off.

Import timing is mixed: the optimized 64 MiB profile uses about 4.8% less CPU,
but its median wall time is 17.9% longer. Individual wall times span 6.16–10.97 s
across these profiles. Preserve that regression and variability in comparisons;
this pass does not establish faster snapshot imports. Database allocated bytes
are essentially unchanged, and apparent bytes are identical. No records or
indexes were removed to obtain the query or executable improvements.

All **1,408,274** measured replies validated with zero errors. This is a shared
exact-AS-key workload, not maximum capacity or a full protocol workload. Public
per-client rate limits are unchanged; these native loopback rates do not describe
what an individual public HTTP client is permitted to send.

## Cold-file-cache and large-policy checks

Only the generated 16 MiB-profile database was fsynced and advised DONTNEED;
Linux mincore verified **0 / 33,027** resident pages before startup. No global
cache drop affected the public services. With the same one-CPU/128-MiB limits,
**161,061** additional responses validated with zero errors. Process PSS was
24.84 MiB and cgroup peak 46.51 MiB, with all memory-limit/OOM event counters
zero. This controls the database file cache, not the entire OS or executable cache.

A separate original synthetic **4,800,056-byte filter-set** exercises a large
policy line. Three imports per binary, alternating order, use a 16 MiB cache and
one CPU under a 256 MiB cgroup. Median CLI peak RSS falls from **25.35 to 24.55
MiB** (about 3.1%). Median wall times are 0.359 and 0.368 s; this short test does
not establish a speed improvement. All six imports verify one complete object
and export byte-identical original text with the standard blank record separator.
The raw JSON records the fixture and export hashes.

Process RSS/PSS excludes unmapped filesystem cache. Cgroup memory includes
charged file cache and helpers; it is a different accounting scope. Neither is
a whole-machine memory reservation or a full Internet-dataset sizing claim.

## Correctness and CI

Local checks passed formatting, strict Clippy, locked all-target tests
(186 passed, one optional skip), 56 external protocol/recovery contracts,
32 report/tooling cases and 18 isolated website cases. The parser fuzzer executed
333,086 cases, including equality of borrowed and owned snapshot parsing.
The final release binary matched **45 TCP and 45 HTTP** cases against official
IRRd 4.5.4 at `8e8080dab744a348b7d0e6efcb53f403301dd075`, using isolated
PostgreSQL and Redis. No updates were sent to public registries.

The unchanged lockfile audit reports no vulnerabilities or advisory warnings,
using freshly fetched RustSec commit `965bf9ad083b58e6340d65829d85217b18be9290`.

Passing CI for runtime `d1d91ca`:

- [Core, Linux x86-64/ARM64, Windows, coverage and audit](https://github.com/RIRRd/RIRRd/actions/runs/41904619109)
- [Pinned upstream oracle](https://github.com/RIRRd/RIRRd/actions/runs/41904619200)
- [Router and bundled dashboard](https://github.com/RIRRd/RIRRd/actions/runs/41904619117)
- [Deployment smoke](https://github.com/RIRRd/RIRRd/actions/runs/41904619181)
- [RPSL/NRTMv3 fuzzing](https://github.com/RIRRd/RIRRd/actions/runs/41904619100)
- [Source publication hygiene](https://github.com/RIRRd/RIRRd/actions/runs/41904619179)

These checks cover the exercised behavior. Full IRRd parity, maximum public
capacity, long-duration mixed traffic and constrained-host qualification on a
complete real-world dataset remain open. Main, release tags and production
promotion are separate from this development checkpoint.

## Reproduce

Save the baseline binary before building the candidate. Generate the fixture
with `tools/bench/generate_fixture.py` outside Git, then on a host with the
recorded CPUs available:

```sh
cargo build --locked --release --bin rust-irrd --example registry_load
python3 tools/bench/run_small_host.py --state /private/optimization \
  --fixture /private/synthetic-100k.rpsl \
  --before /private/baseline-rust-irrd \
  --before-commit e16b0ce4103a33ce010b3a33522ce125640b348e \
  --before-cache-mib 64 --candidate "$PWD/target/release/rust-irrd" \
  --commit "$(git rev-parse HEAD)" --whois-port 9843 --http-port 9880 --seconds 10
python3 tools/bench/check_cold_small_host.py --run /private/optimization/measured-RUN \
  --binary "$PWD/target/release/rust-irrd" --output /private/optimization/cold.json
```

For the large-policy fixture, write `filter-set: FLTR-PERF\nfilter: ` followed by
`AS4200000000 OR ` repeated 300,000 times, then
`AS4200000001\nsource: PERF\n`. Use the same import measurement helper emitted by
`run_small_host.py`, fresh databases, a 16 MiB cache and the stated cgroup limits.
Verify and export each complete object through the normal CLI before counting
the run as valid.
