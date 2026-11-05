# Full-catalog storage checkpoint — 5 November 2026

The development node now serves a verified packed database with **21 sources,
19 object classes and about 14 million objects**. File length fell from
**28.00 to 20.00 GiB** (28.6%).
This preserves the existing table types and uncompressed RPSL representation.
[Machine-readable measurements](results/storage-full-2026-11-05.json) ·
[Operator guide](../operations/STORAGE_MAINTENANCE.md)

| Measurement | Original | Verified packed snapshot | Serving after catch-up |
|---|---:|---:|---:|
| File length, GiB | 28.00 | 20.00 | 20.00 |
| Allocated filesystem blocks, GiB | 27.09 | 19.65 | 19.67 |
| Objects in stable read snapshot | 13,983,606 | 13,983,606 | Live updates continue |

The controlled snapshot saves **27.5% of allocated disk space**.
Original byte counts were sampled before the maintenance restart; object/hash
counts cover the stable read transaction. After normal incremental catch-up,
the serving node reported 13,983,866 objects. The increase reflects
upstream updates, with the frozen snapshot's records preserved during repack.
The logical digest describes the frozen snapshot, before later feed edits.

## Correctness and operation

The command copied every key/value of all 15 application tables,
including object text, all indexes, class counts, mirror cursors/filters,
journals, authoritative audit/protected-name state and signed NRTMv4 state.
It hashed the stable source while copying, compacted and closed the output,
reopened it, ran the full object/index verifier and independently compared its
canonical logical SHA-256:

```text
b1dce95199c49e86bdc20055e3e049b81c8adfcdaa8517dd093aa110674597bf
```

A private staging file was published only after verification and hash equality.
The normal daemon kept serving and mirroring during this online rebuild. A
preliminary raw-copy attempt was abandoned because cold fragmented-file reads
kept development offline too long; development was restored and the in-process
snapshot path replaced it. Production services were not restarted.

The isolated standby then caught up all 14 configured NRTMv3 feeds. Checks
through normal TCP WHOIS, HTTP and GraphQL covered representative objects from
all 19 classes, byte-exact TCP framing, identical TCP/HTTP payloads, credential
redaction, current class totals and the expected 54 AS6939 IPv6 prefixes.
The selected generation passed the same checks after a real daemon restart and
a further polling cycle for every incremental feed. All 21 sources remained
present. This does not establish full IRRd feature parity.

HTTP 200, trusted TLS and the 54-prefix response were verified through public
DNS/Caddy over IPv4 and IPv6 **from the VM**. The independent web-fetch tool could
not access the domain; external IP-family reachability is not claimed here.
The development dashboard reports the new file length.

The original complete database remains available for rollback. Original
snapshot/download receipts and build inputs are retained. A prior operational
change to CANARIE's mirror peer was reconciled against the recorded active
catalogue: snapshot URLs, serial publication and retained classes were unchanged.
The new derived receipt records that provenance explicitly.

## Resources and limits

The VM exposed 24 logical CPUs and
62.73 GiB RAM on Debian Linux/ext4.
The rebuild took 64.94 minutes while normal mirror
workers and sampled queries continued. Query cache remained 64 MiB; the
maintenance destination cache was 1,024 MiB. Batches remained bounded to 1 MiB
or 4,096 rows, allowing one oversized row.

The daemon plus maintenance job peaked at
1387.7 MiB process RSS.
The temporary 32 GiB service limit included kernel filesystem cache and is not
an idle-memory measurement. The ordinary 3 GiB service limit and 30-second stop
timeout were restored. A post-cutover representative-query sample reported
22.4 MiB process RSS; it is not a ceiling under
sustained load.

All 248 sampled loopback status requests during the
online rebuild succeeded. These small status requests are liveness evidence,
not a WHOIS/GraphQL throughput or tail-latency benchmark. No new full-catalog
performance comparison against upstream IRRd is claimed.

The packed object table still contains page slack; larger overflow values are
uncompressed. More storage savings require further measured work. File length
and allocated blocks are reported separately, and incremental updates may grow
either. Runtime data, fixtures, caches and credentials remain outside Git.

## Code and automated evidence

Runtime source: `a0d6e0c289faa25fa57f24336763f55438696a50`. Its Rust, lockfile and bundled
web sources are identical to verified CI commit `3cb679916a1a2b11332ec55378fa7758f10f0a9a`; the latter limits the
SIGTERM-specific shutdown test to POSIX because Windows process termination
uses a different mechanism.

Local gates passed format, strict Clippy, locked tests (190 Rust passes and one
optional skip), 59 protocol contracts, 32 report tests and dependency audit.
The private deployment suite passed 44 tests, including safe selection,
retention and snapshot-publication retry behavior. Deployment code remains in
the private website repository.

- [Linux x86-64/ARM64, Windows, coverage and dependency audit](https://github.com/RIRRd/RIRRd/actions/runs/41986891905)
- [Pinned official IRRd oracle](https://github.com/RIRRd/RIRRd/actions/runs/41986892087)
- [Router integration and bundled dashboard](https://github.com/RIRRd/RIRRd/actions/runs/41986891973)
- [Deployment smoke checks](https://github.com/RIRRd/RIRRd/actions/runs/41986891966)
- [RPSL/NRTMv3 fuzzing](https://github.com/RIRRd/RIRRd/actions/runs/41986891960)
- [Publication hygiene](https://github.com/RIRRd/RIRRd/actions/runs/41986892068)

All six linked workflows passed. Work is on `dev`; main, release tags and
production code remain unchanged. Beta and full-parity qualifications remain
as documented in the compatibility matrix.

## Reproduce on your own data

Use the operator guide's offline command, or launch a maintenance restart with:

```sh
rust-irrd --config irrd.toml server --repack-to /private/candidate.redb \
  --repack-cache-mib 1024
```

Choose a maintenance cache budget appropriate to your host. Keep the output
private and separate, allow space for both files and concurrent updates, and
remove these one-shot options from future starts. Require full verification,
logical-hash equality, mirror catch-up and normal-interface checks before
selecting the candidate. Keep the original for rollback. Do not copy a writable
raw redb file or force-kill an active maintenance writer.
