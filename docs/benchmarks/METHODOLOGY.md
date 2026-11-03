# Benchmark methodology

Use the pinned [IRRd compatibility baseline](../compatibility/IRRD_FEATURE_MATRIX.md)
for comparisons. Check query equivalence and source visibility before recording
performance. Treat the benchmark figures in the project brief as hypotheses.
Report each unsupported workload as unmeasured and cite the missing prerequisite.

## Dataset and environment

- Capture an operator-approved, representative mixture of public IPv4 and IPv6
  snapshots from multiple sources, including routes, deep sets and reverse
  references. Record source, retrieval date, licence and redistribution terms,
  compressed and decompressed bytes, SHA-256, and class and source counts.
  Keep credentials and private data out of fixtures, logs and reports.
- Replay a fixed journal of authenticated or mirror updates and a fixed sequence
  of validator datasets with recorded timestamps and hashes. Verify each
  final object set, visible route set, journal position and rejected-object count.
- Record CPU and core topology, CPU limits, memory, OS and kernel, filesystem,
  disk model and interface, network path, compiler, dependency lockfile, both
  revisions and build modes. Record PostgreSQL/Redis versions and settings for
  the upstream system and the persistence and sync settings of the Rust system.
- Use the same hosts or identically provisioned systems. Isolate load
  generation when possible and verify the client has spare capacity. Record
  placement and TLS, proxy and connection reuse settings. Report whole-stack
  measurements including any database, preload service or reverse proxy in scope.
- Match source policies, object suppression, query limits, journaling and
  durability guarantees. Include index and preload completion in readiness.
  Separate cold host-cache runs from warm runs and document how cache state was
  established. Record every per-system tuning change.

## Workloads

| Workload | Procedure and measurements |
| --- | --- |
| Cold and warm snapshot import | Import the same uncompressed snapshot into a clean database for each repetition. Run with both cold and prewarmed filesystem caches. Measure start to committed source and query-ready state, accepted/rejected records per second, CPU, peak memory and bytes on disk. |
| Compressed snapshot ingestion | Repeat using the identical gzip artifact. Report local-file and controlled HTTP transfer separately, with compressed bytes, throughput and CPU. |
| Incremental updates | Replay the same ordered stream of inserts, replacements and deletes. Verify serial and object parity. Measure acknowledgement/commit latency, lag, write rate and durable bytes. |
| WHOIS exact and inverse | Use fixed traces of hot keys, cold keys, misses and inverse attributes with representative cardinalities. Exercise one-shot and persistent connections independently. Check both record sets and expected protocol framing. |
| Route, prefix and origin | Exercise route and route6 keys, exact, closest covering, all covering and more-specific queries, origin IPv4/IPv6, source restrictions and realistic response sizes. |
| Set resolution | Exercise direct AS-SET membership, recursive expansion, exclusions, implicit membership, cycles and mixed-source sets at several depths and fan-outs. Check correctness before comparing. |
| GraphQL | Include object, status, ASN prefixes, set expansion and relationship lookups with realistic field selections and result sizes. Include batches or fragments only where their semantics and limits match. |
| Concurrent mixed traffic | Run a deterministic weighted mix with source and key skew, hit rate, write ratio, request type, response size and connection reuse recorded. Publish per-class latency alongside aggregate results. |
| Traffic during full imports and NRTM updates | Establish query-only baseline, begin timed import or delta replay and mark commit boundaries. Sample before, during and after. Capture latency degradation, stale-read windows and lag. |
| RPKI updates | Replay matched validator snapshots with representative additions, removals and changes in coverage. Measure time to consistent visible results, suppression journal changes and impact on query latency. |
| Startup and crash recovery | Measure process start to successful readiness plus representative query. Separately measure recovery after controlled termination during acknowledged/unacknowledged writes and during import. Verify durability and source state before counting the run. |
| Resource footprint | Measure idle memory after readiness and quiescence, peak and steady-state memory and CPU under sustained load, total persistent bytes after import and after update churn, and any temporary peak during import/compaction. |
| Long-duration stability | Run the mixed workload with periodic snapshots, delta replay and ROA changes for at least 24 hours on a dedicated runner. Record memory trend, handles, open connections, tasks, database growth, latency and errors over time. Document run length and any interruption. |

## Load and timing

1. Establish a correctness oracle from a pinned reference deployment. Validate
   a representative response sample and deterministic boundary cases at each
   stage. Use full-response reads, including framing and complete payloads.
2. Sweep concurrency at 1, 8, 32, 128 and 256 connections or until saturation.
   Run open-loop offered-load sweeps to capture queueing latency and closed-loop
   saturation tests for capacity. Include client scheduling delay in
   open-loop latency and report offered versus achieved request rates.
3. Start with 60 seconds of warm-up, five minutes of steady-state collection
   and five repetitions per point. Record the actual timings and extend runs
   that fail to stabilize. Alternate run order across systems and reset state
   when the workload changes it.
4. Report p50, p90, p95, p99 and maximum end-to-end latency per workload at each
   load, successful throughput, timeout and error counts, response bytes,
   CPU, memory and storage. Keep timeouts in the error accounting and specify
   how censored observations are handled. Report per-run values, median and
   variance across repetitions. Use monotonic clocks for elapsed times.
5. Retain the command line, workload seed or trace, configuration with
   secrets removed, raw timing and metrics, artifact hashes and analysis
   script. Investigate discrepancies and rerun correctness checks after
   changing dependencies, durability or indexes.

A synthetic smoke run verifies the harness and provides a local development
checkpoint. Establish performance claims with a real corpus and a configured
upstream run using the procedure above.

## Running exact-key WHOIS differential checks

Load [the synthetic RPSL fixture](../compatibility/differential-whois.rpsl) into
two isolated test deployments with the `DIFFERENTIAL` source enabled. Start the
pinned upstream and current Rust candidate on distinct local ports. Configure
the same default source and disable RPKI, scope and preference filtering for
this fixture. Use operator-approved endpoints. The
[Docker Compose oracle](../../tools/oracle/README.md) provisions the pinned
reference, PostgreSQL and Redis with the fixture loaded.

```sh
python tools/differential_whois.py --reference 127.0.0.1:8043 --candidate 127.0.0.1:9043 --allow-filtered-auth-omission
```

The built-in corpus expects hits and misses and exercises canonical and legacy
IPv4 and IPv6 route keys. The tool requires exact status framing, UTF-8 byte
counts and clean connection termination. It compares success and error payloads
byte-for-byte by default and returns exit status 1 for any mismatch. The
fixture uses a synthetic password marker. The explicit
`--allow-filtered-auth-omission` flag permits only omission of upstream's known
password-redaction placeholders. `--normalize-attribute-padding` permits
differences in whitespace immediately after attribute separators. Both options
apply only after framing validation. The tool does not print response content.

To supply other cases, pass `--corpus` with an ASCII file containing one expected
status (`A`, `C`, `D` or `F`), a tab and a literal `!m` query per line. Blank
lines and lines beginning with `#` are ignored. Keep this check separate from
latency measurements.
