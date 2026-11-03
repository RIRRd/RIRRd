# Continuous mirroring and the registry console — 3 November 2026

This checkpoint continues the [twelve-source checkpoint](EXPANDED_SOURCES_CHECKPOINT.md).
The public read-only test service is https://rirrd.co.uk/dashboard/ and
`rirrd.co.uk:43`. Full IRRd parity and production qualification remain incomplete.

## What “live” means

A snapshot is the initial state and a recovery mechanism. A supported source should
then consume incremental changes from the serial associated with that publication.
A daily download alone can lag the authoritative source by a day or more. Page
telemetry refreshing every 15 seconds does not establish registry-data freshness.

The source catalog configures five feeds from Merit's matching publications:
RADB, ALTDB, BELL, REACH and BBOI. The builder reads each snapshot serial before
and after downloading its associated snapshot, rejects publication races, imports
routing classes and catches up over NRTMv3 before activation. The daemon polls
each configured peer every 60 seconds. No unverified current serial is substituted
for the snapshot serial. The resolved public peer address is pinned in the runtime
configuration; re-resolution occurs at the next full rebuild.

[Merit's NRTM documentation](https://radb.atlassian.net/wiki/spaces/RADb/pages/46105414/NRTM)
identifies its public endpoint and describes upstream propagation delay. A
successful poll means the peer supplied a complete recognized response; it does
not guarantee immediate visibility of an authoritative edit. NRTMv3 and the
configured anonymous FTP bootstrap are unauthenticated, unencrypted transports.
Source terms continue to apply independently of the MIT code license.

## Protocol and storage changes

Real RADB responses omit some operation serials. The official v4.5.4 parser at
`8e8080dab744a348b7d0e6efcb53f403301dd075` accepts ordered NRTMv3 omissions and
records the declared terminal serial. The new filtered profile matches that
behavior, including leading/trailing omissions and a range with no visible
operations. It rejects duplicate/backward/out-of-range serials, truncated framing,
wrong trailers/sources, unsafe controls and invalid objects. Five original
synthetic framing/omission vectors are checked against upstream. These are not
complete protocol parity evidence.

The filtered profile permits snapshot-sized policy objects up to 4 MiB inside
an overall 64 MiB/15-second scheduled-fetch ceiling. The existing unfiltered
profile retains its contiguous-serial and 64 KiB admission behavior; this is a
remaining deliberate restriction, not complete NRTMv3 parity. The new parser has
a dedicated fuzz target for both profiles.

Every visible operation is parsed and source checked before admission. Only
configured classes enter storage. Excluded contact, maintainer and other records
are not persisted or added to a hidden journal. A partial mirror cannot safely
republish a complete upstream stream, so its journal is disabled and filtered
sources cannot be configured with a downstream NRTM ACL. Its persistent class
policy cannot change without an explicit snapshot reset. The integrity checker
validates policy/cursor/object membership and absence of a partial journal.
A complete no-new-updates reply also persists a quiet source's verified bootstrap
cursor and class policy. Failed or malformed responses retain the previous state.

The public status reports poll state, last check, last successful check, cursor,
interval and transport limits. Quiet sources have real committed cursors. Status
is reset to unverified at daemon restart until a new successful check. Aggregate
object count is read from the current committed database; per-source class counts
are explicitly the bootstrap snapshot counts and may differ after incremental
changes. No expensive full-dataset recount runs on each dashboard refresh.

## Console changes

The console adds a dedicated API page with live read-only GraphQL queries and
schema inspection, public endpoint examples, covering/more-specific route searches,
RPSL object summaries, per-source class breakdowns, publication receipts, original
terms links and access blockers. All text from objects/status uses text nodes.
Queries stay bounded; preview mode cannot query a registry. No third-party assets,
analytics, public submission endpoint or automatic router changes were added.
Node location/sponsor/contact information is not invented; operator input is pending.

## Evidence and outstanding work

A private normal-CLI replay applied a real RADB response: 6,259,111 bytes,
4,602 operations over advertised serials 6,851,027–6,855,696, producing a verified
private test database. This was a parser/import exercise with a synthetic seed,
not proof of a complete mirror, a benchmark or a replacement for proper bootstrap.
The real transcript, snapshots, databases and logs remain outside Git.

The final deployment/verification results are appended below after activation.
No new private GitHub Actions result is claimed: the deploy key cannot verify
Actions runs or repository settings. Local commands and recorded outputs provide
the evidence for this checkpoint.

Other feeds remain snapshots until a matching bootstrap, admission policy and
incremental path are verified. RIPE's currently observed official NRTMv4
publication is ES256 with a separately published P-256 key; its web manual also
mentions Ed25519, so that wording alone must not determine runtime compatibility.
The observed notification had 1,055 retained deltas, exceeding this implementation's
512-selected-file transaction budget if all are needed. Scheduled filtered NRTMv4,
catch-up batching, signed publication and rollover/recovery remain open. LACNIC's
HTTPS snapshot-serial publication and NRTM endpoint are accessible; safe activation
is a subsequent integration task. AFRINIC's NRTM whitelist and the five pending
source-access/connectivity actions from the previous checkpoint still apply.

Remaining parity includes OpenPGP/key-cert, accounts/tokens/mail/overrides,
remaining authoritative classes, broader query and policy behavior, continuous
signed NRTMv4, bidirectional replication and release qualification. Read-only
experimental access does not constitute production readiness.
