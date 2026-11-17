# Source freshness and recovery

Beta.5 contains the public daemon fix for filtered RIPE NRTMv3 feeds stopping
on `poem` or `poetic-form`. Beta.4 and earlier release packages do not contain
that fix. Upgrade before using a standard-class filtered RIPE feed. A version
number in a checkout does not establish which binary your service is running;
check `rust-irrd --version`, its service executable and the release checksum.

## What the package provides

The daemon imports local snapshots and can poll configured NRTMv3 feeds in
process. It validates complete responses before committing objects and serials
together. Its existing polling interval retries failed attempts without marking
the failed attempt as a successful update. Initial snapshots, their verified
starting serials and snapshot-only refresh scheduling remain operator tasks.
The package does not include the community site's source catalog, download
service, mirror selection or generation activation scripts.

On filtered RIPE streams, beta.5 validates poetry extensions before discarding
them through the configured standard-class filter. Later supported records and
the cursor can commit. This does not accept arbitrary unknown classes, suppress
source/framing errors, or permit poetry objects in authoritative submissions or
strict snapshot import. Unfiltered readers continue to reject these classes.
All nineteen standard classes remain supported for mirrored storage.

For a full standard-class RIPE mirror, set an explicit retained-class filter:

```toml
[nrtm3_sync.RIPE]
endpoint = "192.0.2.10:43"
interval_seconds = 60
object_class_filter = [
  "as-block", "as-set", "aut-num", "domain", "filter-set", "inet-rtr",
  "inet6num", "inetnum", "irt", "key-cert", "mntner", "organisation",
  "peering-set", "person", "role", "route", "route-set", "route6", "rtr-set"
]
```

Replace the documentation address with your actual peer's IP and port. The
source must also appear in the top-level `sources` list and already contain a
matching initial snapshot. Preserve an existing committed serial. On first
bootstrap only, configure `base_serial` from that snapshot's verified serial;
do not guess it. A narrower class filter must match the stored snapshot scope.
Upgrading a deliberately unfiltered configuration alone does not enable this
filtered behavior. Do not configure complete NRTMv3 journal serving from a
filtered mirror, because its retained data omits upstream operations.

## Monitor data freshness

Monitor each source separately: import timestamp, retained class counts, last
successful feed check, committed serial, upstream availability and poll errors.
A successful HTTP health check, an active timer or an unchanged object count
does not prove that a snapshot refreshed. A no-update reply can legitimately
leave the serial unchanged. Snapshot-only feeds should retain their real import
date and show an overdue warning when the expected interval is missed.

The hosted nodes' 17 November recovery verified 14,006,896 objects and indexes
before activation: 21 datasets, fifteen incremental feeds and six daily
snapshots. This is evidence for that prepared image, not a guarantee that every
upstream will remain available. Consult live telemetry for subsequent status.

## Snapshot refresh safeguards

Use these safeguards in your own automation:

1. Acquire a per-profile lock. Check space for the candidate, serving database,
   retained rollback and temporary files before starting. Measure the actual
   profile; a full catalog requires much more disk than a small source subset.
2. Build an independent candidate. Never import into or copy a database that
   another process has open for writes. Keep inputs and staging private.
3. Retry transient transport failures a finite number of times with delay and
   cleanup of partial downloads. Certificate, validation and checksum failures
   must fail the build. Do not advance a serial to conceal a failed import.
4. Verify the publisher's serial before and after fetching a matching snapshot.
   Secondary mirrors can have different serial namespaces. Replacing a mirror
   requires confirming identity, class scope, policy filtering and serial
   provenance; availability alone does not establish equivalent data.
5. Validate configuration, source/class counts and loss thresholds, then run
   `database verify` against the inactive candidate. Account for verification
   memory separately from the serving cache budget.
6. Flush the prepared image and retain a known-good rollback. Switch only after
   verification, then check native queries, source status and newly exported
   import timestamps. Allow monitoring exports to publish the new generation
   before declaring success. Restore the previous generation on failed checks.
7. After failure, retain small diagnostic receipts and prune only abandoned
   candidates owned by this updater. Use a grace period and lock; protect the
   current, verified and rollback generations. Never delete files by age alone.

Our separately maintained hosted updater implements bounded network retries,
failed-staging cleanup and guarded activation. Its 17 November recovery also
replaced an unavailable LEVEL3 mirror endpoint. Those operator changes are not
features added to the public daemon package. Endpoint reachability, disk
exhaustion and future unsupported publisher data still need monitoring.

## Tests and upgrade

The public regression `filtered_ripe_extensions_keep_framing_and_source_checks`
runs with `cargo test --locked --all-targets` on every core CI build. It covers
both poetry classes, strict-reader rejection, filtered parsing, committing the
following standard record with serial advancement, wrong-source rejection and
incorrect trailer rejection. The retained database passes native integrity
verification. Broader protocol and pinned official-oracle checks remain separate.

The external protocol suite also runs
`test_filtered_ripe_extensions_sync_cli_rollback_and_restart` on every core
build. It serves a synthetic loopback peer to the normal `source sync-nrtm3`
CLI, commits both excluded classes followed by a retained route, rejects a
malformed follow-up without changing the cursor or records, verifies the
database, and checks TCP/HTTP answers across two daemon starts.

Passing CI for the original fix `d4a595c6d7316b555abaac4de16e9a473074e917`:
[core Linux/ARM64/Windows, audit, browser and coverage](https://github.com/RIRRd/RIRRd/actions/runs/44041590489),
[pinned official oracle](https://github.com/RIRRd/RIRRd/actions/runs/44041590625),
[fuzzing](https://github.com/RIRRd/RIRRd/actions/runs/44041590554).
Check the beta.5 tag's release workflow for the final package qualification.

Back up the database, configuration and old binary while stopped before
installing the new package. There is no storage-format change in this patch.
After restarting, confirm the affected source advances and representative
queries still return complete answers. If the peer has expired the old serial,
bootstrap from a current matching snapshot instead of inventing a new cursor.
See [source profiles](SOURCE_PROFILES.md) and [installation](GETTING_STARTED.md).
Full IRRd parity and production qualification remain incomplete.
