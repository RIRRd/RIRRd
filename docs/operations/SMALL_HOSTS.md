# Smaller-host operation

RIRRd's target is the same registry feature set on smaller hosts, not a reduced
feature edition. Full parity remains incomplete. Tune budgets and source scope;
do not disable authentication, durable commits or validation to improve a chart.

The [evaluation profile](../../examples/small-host/irrd.toml) configures:

| Control | Evaluation value | Meaning |
| --- | ---: | --- |
| `database_cache_mib` | 16 | redb cache budget, not total RAM |
| `worker_threads` | 1 | Tokio runtime workers |
| `max_blocking_threads` | 4 | Blocking query/import worker cap in the running daemon |
| `max_connections` | 8 | Concurrent WHOIS clients |
| `max_connections_per_ip` | 8 | Per-IP WHOIS cap, within the global cap |
| `max_snapshot_bytes` | 16 MiB | Per-input uncompressed size bound |
| HTTP / WHOIS | Loopback | Private evaluation interfaces |

The normal default database cache is now **64 MiB**, replacing the database
library's implicit 1 GiB default. An explicit `database_cache_mib = 1024` restores
the earlier cache budget. The setting applies to daemon startup, imports,
verification, compaction, backup/restore and mirror commands. It changes cache
capacity, not on-disk schema or durability. Worker settings are optional; omitted
values preserve runtime CPU detection/default blocking-pool behavior.

A cache budget is not a process-memory ceiling. Runtime stacks, connection and
response buffers, indexes, policy feeds and temporary update/import work also
consume memory. A cgroup limit additionally accounts for charged filesystem
cache. Monitor process memory and cgroup memory separately. Small caches can
increase I/O and latency when the working set grows; measure your own workload.

## Start with the source data you need

Pin a trusted source list appropriate to your peers. Size licensed snapshots,
disk allocation, journals, backups and temporary staging before importing.
The bundled synthetic fixture is small; its successful run does not prove that
a full Internet registry or arbitrary AS-set fits the same host.

Avoid compiling or fuzzing during load measurements. Publish the exact binary,
source revision, fixture hash, CPU limit, request concurrency, duration, error
counts and memory-accounting scope. The [earlier 100k comparison](../benchmarks/LINUX_SYNTHETIC_100K.md)
used a much larger server CPU allowance and must not be relabeled as a minimum
host requirement.

## Use systemd limits deliberately

For an evaluation service, a reviewed drop-in can bound resources, for example:

```ini
[Service]
CPUQuota=100%
MemoryHigh=192M
MemoryMax=256M
TasksMax=64
```

These are experimental bounds, not a supported production minimum. Verify imports,
queries, updates, mirror recovery and shutdown within the chosen limits. An OOM
kill is a failed test, not a successful memory measurement. Preserve a verified
backup and leave operational headroom; do not apply a new memory cap blindly to
a live registry.

The bundled dashboard adds no Node runtime. It loads local static assets and
refreshes source status on request, with bounded queries through the existing
loopback API. It reports configured budgets and available replication metadata;
it does not invent process-memory or source-freshness readings.

See [router integration](ROUTER_INTEGRATION.md) for the private evaluation workflow.
