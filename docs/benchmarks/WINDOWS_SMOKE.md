# Windows smoke benchmark, 3 November 2026

The [raw report](windows-smoke-2026-11-03.json) records the release binary and dataset hashes, three repetitions, elapsed times, response latencies and throughput. The host ran Windows 11 Pro for Workstations build 26200 with an Intel Core i5-11600K (6 cores, 12 threads), 64 GiB RAM and a Fanxiang S880 1 TB NVMe SSD. The database was on NTFS. The compiler was Rust 1.99.1 for `x86_64-pc-windows-gnu` using Cargo's release profile. Python 3.12.14 and the server shared the host.

```sh
cargo build --locked --release
python tools/benchmark_smoke.py --binary target/release/rust-irrd.exe --objects 10000 --queries 4000 --warmup-queries 400 --concurrency 1 8 32 --repetitions 3
```

Each timed query made one IPv4 loopback TCP connection, issued an exact ASN lookup and validated the full response. The benchmark used a deterministic synthetic dataset. The OS page cache and other host activity were uncontrolled.

| Workload | Wall time range | Database file length |
|---|---:|---:|
| Plain import to a fresh database | 0.205–0.604 s | 4.75 MB |
| Plain replacement | 0.477–0.602 s | 135.28 MB |
| Gzip replacement after plain replacement | 0.441–0.508 s | 71.05 MB |

| Concurrent clients | Mean queries/s | Sample standard deviation, queries/s | p95 range |
|---:|---:|---:|---:|
| 1 | 1,260 | 357 | 0.55–0.83 ms |
| 8 | 10,903 | 891 | 0.88–1.14 ms |
| 32 | 9,713 | 3,890 | 3.76–4.14 ms |

File length increased by about 28.5× after plain replacement. Measure allocated blocks and redb page statistics over repeated replacements before choosing an optimization. The benchmark did not measure process memory, CPU, controlled cold-cache imports, representative traffic or upstream IRRd. Use the [comparative methodology](METHODOLOGY.md) for release qualification.

## Query expansion checkpoint

The release build at `e215aa5` was measured on the same host with the same command. The [second raw report](windows-query-expansion-2026-11-03.json) records the new binary hash and per-run samples. The workload still exercises bare ASN lookups. It does not measure the new prefix, inverse, set or GraphQL queries.

| Concurrent clients | Mean queries/s | Sample standard deviation, queries/s | p95 range |
|---:|---:|---:|---:|
| 1 | 1,380 | 329 | 0.54–0.74 ms |
| 8 | 10,745 | 1,406 | 0.90–1.26 ms |
| 32 | 5,579 | 1,280 | 4.46–6.06 ms |

Fresh import took 0.268–0.369 s, plain replacement 0.541–0.674 s, and gzip replacement 0.459–0.478 s. Observed file lengths were unchanged. The 32-client run was slower than the foundation checkpoint. Repeat alternating builds on an isolated host and profile the client and server before attributing the difference to a code path.

## Offline compaction experiment

The [raw compaction report](windows-compaction-2026-11-03.json) records one debug-build run with the same 10,000-object input. Two plain imports produced a 135,278,592-byte file. With the daemon stopped, `database compact --yes` reduced its length to 11,837,440 bytes in about one second, including logical verification. A separate `database verify` process reopened the file and confirmed all 10,000 objects. The 91.25% reduction applies to this single synthetic file. Replacement still has the same peak file length in this run.
