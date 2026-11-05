# Database storage maintenance

These commands are introduced in beta.3. Earlier releases do not provide
verified repacking; check `rust-irrd --version` before following this guide.

The `database` subcommands require an **offline database or inactive generation**. The
normal exclusive database lock rejects access to a serving instance. Use a
private directory, keep a verified original for rollback and allow space for a
complete second database. Repacking does not activate or replace a database.

```sh
rust-irrd --config /private/offline.toml database stats
rust-irrd --config /private/offline.toml database count --by-class
rust-irrd --config /private/offline.toml database repack --file /private/packed.redb
```

`offline.toml` must point to the offline source file and pass the normal
configuration checks. All sources and application tables are processed,
independently of the configuration's query source selection.

## Measure occupied space

`database stats` reports keys/values, metadata, fragmentation and entry counts
for each table. It can scan substantial data and is not an integrity verifier.
Its output contains table metadata, without object text or credentials.

`file.apparent_bytes` is the file's reserved logical length, as shown by `ls -l`.
On Unix, `file.allocated_bytes` uses allocated 512-byte filesystem blocks, as
reported by `stat`/`du`. Other platforms report allocated bytes as `null`.
Sparse files can have the same apparent length while occupying less disk.
Database-level `fragmented_bytes` includes free preallocated pages as well as
unused room inside pages; it is not a promise that all those bytes can be removed.
Per-table figures help distinguish data/index costs from free file capacity.

`database count --by-class` reads the committed per-source/class counters. It
does not scan or verify index integrity; its JSON explicitly reports
`integrity_verified: false`. The default `database count` output is unchanged.

## Repack without changing the storage format

`database repack --file NEW_PATH` creates a separate database using the existing
table types and uncompressed RPSL representation. It copies **every key/value**
of every supported application table, including:

- Original object text and every lookup index/class count.
- Mirror cursors and class filters, NRTMv3 journals and authoritative serials.
- Signed NRTMv4 client state, authoritative audit history and protected names.

The copy disperses insert order inside bounded batches to reduce half-filled
B-tree pages. This is a physical packing strategy; it does not change query
ordering, source priority, records, policy or credentials. Each batch holds up to
1 MiB or 4,096 rows, with room for one larger row. Even a large multimap value
group is streamed in bounded batches. Source and output each use the configured
database cache budget; total process/kernel memory is not limited to one cache.

The command hashes every source key/value in canonical order while copying,
before shuffling each bounded batch. It includes length-delimited table
identity and row counts. After compaction and closing, it reopens the final
artifact, runs full index/object verification and independently hashes its
contents for comparison with the stable source snapshot. Publication requires
both checks to pass; corrupt source indexes cannot produce an accepted copy.
This avoids redundant full-database walks during large maintenance jobs. A private staging file is synced
and published without overwriting an existing destination. Failed preparation
removes its staging file; the original remains available.

Unknown application tables and persistent redb savepoints are refused, rather
than omitted. The output includes the object/table counts, logical SHA-256,
elapsed time and source/output apparent and allocated bytes. It remains a raw,
credential-bearing database; the public query redaction rules still apply.

Validate representative TCP/HTTP/GraphQL queries and normal updates against the
new file before selecting it in your deployment configuration. Preserve external
configuration, trust keys and the original database for rollback. The current
storage format remains readable by the previous runtime; no codec migration is
introduced by this operation.
The output belongs to the user running the command. Before activation, give the
intended daemon account read/write ownership and access through its private
parent directory; retain restrictive file permissions.

## Repack while serving

A maintenance restart can launch a bounded background repack in the process
that owns the serving database:

```sh
rust-irrd --config irrd.toml server --repack-to /private/candidate.redb \
  --repack-cache-mib 256
```

The daemon keeps its configured query cache. The optional budget applies to the
candidate only; it defaults to the configured cache size. Listeners and mirrors
continue running. One stable read transaction covers all source tables and
mirror cursors, even when updates commit during copying. Final verification and
logical hashing still run before publication. Completion or failure is logged;
the candidate is never selected automatically and the serving file stays in use.

Long read transactions retain older pages needed by the snapshot. Allow space
for both files and concurrent updates, and account for maintenance I/O and cache
memory separately from ordinary queries. Use an absolute, private output path
writable by the daemon account. Do not leave the one-shot options in a permanent
service command: a later restart must not try to overwrite the finished file.

Shutdown requests cancellation during table and index scans, then waits for
clean database closure. Engine compaction and fsync are not interruptible; give
maintenance shutdown sufficient time and never force-kill an active writer.
Before activating the candidate, catch up its preserved mirror cursors and
validate normal queries. Keep the previous generation for rollback.

## Compaction and trade-offs

The existing `database compact --yes` reclaims movable unused space in place,
with full verification before and after. It can help after purges/replacements,
but does not generally repack underfilled live B-tree leaves. A fresh import can
therefore see little improvement from ordinary compaction alone.

Repack savings depend on insertion order, object sizes and update history.
Already dense databases can see little saving or grow. Large overflow values
are not compressed. Future incremental updates can use free space or split
pages again. Compare measured disk blocks and query/update behavior on a
candidate; do not treat a synthetic percentage as a full-catalog prediction.

The [storage checkpoint](../benchmarks/STORAGE_2026_11_04.md) records the measured
fixture, limits, correctness checks and current deployment scope.
