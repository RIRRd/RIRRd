# One-CPU small-host checkpoint — 3 November 2026

Candidate source: `b3d8ad352205e938baffd2fdb078c112da012dd7`; binary SHA-256 `2a3d962ddf462f7438e630c142641d99cfe54f05f9efc43862782a521de553a7`. Previous binary: `92b3f9f42b09549b7006e2068da69ce05ec0b4e7`. Both exact binary hashes and the validating client hash are in the raw results.

The original 100,000-object PERF fixture is 15,485,046 bytes, SHA-256 `9fe1bf623acb0499df78537f4d097582c0fd1bba0fa4c99ae4b21a38dee3a0f2`. It is original synthetic data, not a public registry import. The same host is a Debian 13 / 24-vCPU / 62-GiB VM; the following are cgroup-constrained process tests, not a physical minimum-VM qualification.

Servers/imports use CPU 0 and CPUQuota=100%. The native four-client driver uses CPUs 16–19. Imports have MemoryMax=256M; query services have MemoryMax=128M and TasksMax=64. No compiler, fuzz or install workload ran during timing. The public Caddy site remained active.

Three fresh imports per profile and three 10-second reused-connection WHOIS/GraphQL runs per profile, with three-second warmups. Profile order rotates. All imports accept 100,000 objects; every measured query checks framing/identity/source (and GraphQL class/errors). Input cache may be warm. Old/default64/small16 profiles have 1 GiB/64 MiB/16 MiB redb cache budgets. New profiles explicitly use one runtime worker and four blocking workers; the old binary uses CPU detection/default blocking-pool settings. WHOIS connection cap is eight. This compares complete profiles, not cache size in isolation.

| Measurement | Previous defaults | New default, 64 MiB | Small profile, 16 MiB |
| --- | ---: | ---: | ---: |
| Import median seconds | 7.89 | 9.20 | 8.70 |
| Import CLI peak RSS, median MiB | 110.58 | 18.07 | 13.18 |
| Import cgroup peak, median MiB | 209.80 | 117.09 | 112.11 |
| Idle process PSS, median MiB | 33.37 | 30.35 | 22.25 |
| Whois queries/s, median | 12,743.230 | 13,112.914 | 11,606.366 |
| Whois worst per-run p95 ms | 0.610 | 0.600 | 0.660 |
| Whois worst per-run p99 ms | 0.825 | 0.813 | 0.888 |
| Graphql queries/s, median | 4,456.831 | 4,122.838 | 3,953.489 |
| Graphql worst per-run p95 ms | 2.003 | 2.160 | 2.294 |
| Graphql worst per-run p99 ms | 2.719 | 2.880 | 3.147 |

All **1,500,091** measured responses validated with zero errors. All nine bounded imports completed. The new default reduces median import-process peak RSS by about 84%; median import time increases from 7.89 to 9.20 seconds in this fixture. The 16 MiB profile uses still less process memory, with lower median query throughput. Preserve these trade-offs; do not report a universal speed gain.

Process RSS/PSS excludes unmapped filesystem cache. Import cgroup peaks include the measuring Python helper and charged filesystem cache. Query cgroup accounting in the repeated phase can inherit already-cached file pages from a stopped import unit; the separate eviction check below addresses that scope. These are not interchangeable whole-machine RAM figures.

## Per-file cold-start budget check

After the repeated phase, the 16 MiB profile was checked again. Only its generated redb file was fsynced and advised DONTNEED; no global cache drop was used. Linux mincore verified zero resident file pages before restarting the service. Other services and their caches were preserved.

- File residency: 24,913 pages before, **0 / 33,027** after eviction.
- Same one-CPU / 128-MiB service limit; four clients, two additional 10-second workloads.
- After queries: process PSS **22.16 MiB**, cgroup current **45.94 MiB**, cgroup peak **46.45 MiB**.
- Memory events: `{"high": 0, "low": 0, "max": 0, "oom": 0, "oom_group_kill": 0, "oom_kill": 0}`. No OOM or max-limit events.
- **161,810** additional responses validated with zero errors.
- Whois: 12,318.54 queries/s, p95 0.618 ms, p99 0.834 ms.
- Graphql: 3,858.72 queries/s, p95 2.079 ms, p99 2.893 ms.

This recheck controls the measured database file cache. It does not cold-boot the OS, remove shared executable/library pages or validate a complete Internet dataset, update/mirror workload or production minimum. Larger licensed datasets, sustained mixed traffic, authentication load and real constrained-host qualification remain open.

## Reproduce

Generate the original fixture outside Git with `tools/bench/generate_fixture.py`. Preserve the previous binary before building the candidate. Run the root-only systemd controller on this VM with CPUs 0 and 16–19 available:

```sh
python3 tools/bench/run_small_host.py --state /private/benchmark-state --fixture /private/synthetic-100k.rpsl --before /private/previous-rust-irrd --candidate "$PWD/target/release/rust-irrd" --commit "$(git rev-parse HEAD)" --seconds 10
python3 tools/bench/check_cold_small_host.py --run /private/benchmark-state/measured-XXXXXX --binary "$PWD/target/release/rust-irrd" --output /private/cold-result.json
```

The scripts leave databases/logs in their private state directory and stop their transient service units. A complete result requires `completed_at`, successful imports and zero measured query errors. Failed runs must not be substituted into success comparisons.

Raw results: [three-profile run](results/small-host-2026-11-03.json), [cache-eviction recheck](results/small-host-cold-2026-11-03.json). The [small-host guide](../operations/SMALL_HOSTS.md) describes configuration and accounting limits.
