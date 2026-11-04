# Database storage maintenance

These commands require an **offline database or inactive generation**. The
normal exclusive database lock rejects access to a serving instance. Use a
private directory, keep a verified original for rollback and allow space for a
complete second database. Repacking does not activate or replace a database.

```sh
rust-irrd --config /private/offline.toml database stats
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

Before publication the command verifies source indexes, hashes every typed
key/value with length-delimited table identity/counts, verifies the output and
compares the logical hashes. It compacts and closes the output, reopens it and
repeats output verification/hash comparison. A private staging file is synced
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
