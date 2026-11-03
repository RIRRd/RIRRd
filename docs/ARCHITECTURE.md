# Architecture decisions

## Process boundaries

Use a modular monolith with explicit parsing, persistence, import and network boundaries. Tokio manages network I/O. Synchronous database work runs on a blocking executor with a connection permit held for the lifetime of that work, including after a client deadline. The listener stops accepting on shutdown and drains admitted requests. The current offline importer uses the same storage API as the listener. A future scheduler must submit in-process work to a bounded writer queue.

## Storage choice

IRRd 4.5.4 uses PostgreSQL. This branch evaluates embedded storage to reduce the operating footprint for operators. redb 2.6 provides an embedded copy-on-write B-tree, single writer, snapshot readers and synchronous commit without an external service or a native C++ build. This makes it the initial candidate. Reassess the branch and engine choice after realistic dataset measurements. [redb documentation](https://docs.rs/redb/2.6.3/redb/)

| Candidate | Trade-off |
|---|---|
| redb | Pure Rust with transactional tables and snapshot readers. Evaluate bulk load, allocator recovery, file amplification and time pinned by readers. |
| RocksDB | LSM storage and rich bulk-loading controls. Requires more compaction and durability tuning and a native build. |
| LMDB via a Rust binding | Memory-mapped B-tree and cheap readers. Evaluate map sizing, operational handling of long-lived readers and native binding maintenance. |
| SQLite | Widely deployed, transactional and operationally familiar. Evaluate WAL/checkpoint interaction with large snapshots and index layout. |
| PostgreSQL | Matches upstream infrastructure and is useful as a benchmark control. Requires a separate service. |

The object identity tuple is `(source, class, primary key)`. The exact index is updated in the same transaction as object contents. Primary-key matching performs a bounded index read. Imports stream objects into one immediate-durability write transaction. A parse, read, source, or duplicate error aborts replacement. Readers using the same open database can continue to read the previously committed snapshot. Cross-process concurrent opening is outside the current design. Commit errors are indeterminate until reopened and verified. Data-format version suffixes identify tables. A production upgrade and schema-migration policy remains open.

Prefix and inverse indexes will have to publish with their source generation. Measure a sorted prefix range index against an in-memory trie before choosing a design. Origin, maintainer and membership work needs parsed attributes and bounded enumeration. Treat filters and source order as part of query semantics and exercise them in differential tests.

## Initial threat model

| Surface | Current control | Evidence or release work |
|---|---|---|
| Slow or large TCP requests | Fixed line and connection limits, phase deadlines and bounded result counts. | Unit and socket tests. Add fuzzing, sustained overload, IP access policy and Linux shutdown tests. |
| Malformed, compressed or mislabelled snapshots | Bounded decompression and records, source check and transaction rollback. | Gzip limit/trailer and failed replacement tests. Add realistic encoding tests and CPU/disk exhaustion tests. |
| Credential disclosure | Structured service diagnostics, rejected submission pseudo-fields and suppression of sensitive RPSL attributes in public rendering. | Maintainer lookup smoke test. Raw database and input files must be protected by filesystem permissions. Add audit and review all public output paths. |
| Storage or process failure | Immediate-durability commit and logical index verifier. | Reopen/rollback tests. Power-loss, storage exhaustion, physical corruption and restore remain release gates. |
| Remote fetching and signed data | Future network surface. | Define URL/redirect/DNS policy, verify TLS and signed metadata, bound decompression and verify session and serial continuity before applying updates. |
| Authoritative changes and administration | Future write surface. | Define per-source authorisation, transactional validation, request limits, secret lifecycle, audit and authenticated management boundaries. |

## Upstream material

The implementation uses protocol specifications and upstream IRRd as behavioural references. It was written independently. The example route uses documentation address and ASN ranges. See the [pinned upstream licence and sources](compatibility/IRRD_FEATURE_MATRIX.md) before importing any upstream fixture or source material.
