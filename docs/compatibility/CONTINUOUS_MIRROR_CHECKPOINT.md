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

The activated deployment and final verification results are recorded below.
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

## Activated deployment and verification

The generation `20261103T153951Z-nrtm` activated successfully at
**2026-11-03 16:10:22 UTC**. Its bootstrap contained **2,610,760** objects; after
initial incremental catch-up, complete verification accepted **2,610,955**. The
running daemon then advanced to **2,611,104** at the recorded second poll.
These counts are observations, not a fixed advertised live total.

| Feed | First live observed cursor | Result on next scheduled poll |
| --- | ---: | --- |
| ALTDB | 160016 | following, checked 60 seconds later |
| BBOI | 20081 | following, checked 60 seconds later |
| BELL | 251170 | following, checked 60 seconds later |
| RADB | 6856018 | following, checked 60 seconds later |
| REACH | 874055 | following, checked 60 seconds later |

The other seven loaded feeds remain daily snapshots. APNIC/IDNIC agreement,
JPIRR approval, NTTCOM onboarding and LEVEL3 connectivity remain unresolved;
the public source page shows their exact disposition. There is no new public
submission, administration or downstream mirroring endpoint.

A first catch-up attempt retained the old live generation when its network reader's
deadline expired during local indexing. A fresh bounded RADB capture was valid
UTF-8 and transferred 6,713,429 bytes in 1.25 seconds. CLI sync now downloads to
a private bounded file before applying, with an optional network-only deadline;
bootstrap uses 60 seconds, scheduled steady-state workers retain their bounds.
A regression test reads the staged response after its download deadline has passed
and separately rejects oversized transfers. The resumed candidate reused all
12 verified normalized inputs and completed catch-up without redownloading them.

Preparation now uses a clearly labeled non-verifying `database count` probe and
one complete post-catch-up integrity scan. The recorded compatible runtime upgrade
requires the exact original binary fingerprint via `--previous-binary`; ordinary
resume still rejects changed inputs. A final whole-database fingerprint permits
activation without repeating the full scan only when the installed runtime and
all database bytes and the prepared configuration match the final verified report. Different runtimes receive a
full verification; tampered bytes/catalog/counts are regression-tested failures.
The final scan remains mandatory before any prepared generation is publishable.

The initial private build used two CPUs and a 2-GiB ceiling, increased to 4 GiB
for cache-intensive verification (3.2-GiB recorded peak, no OOM). The successful
resume used two CPUs/4 GiB and peaked at 1.9 GiB; activation peaked at 12.3 MiB.
These private job accounting figures are not whole-host minimum requirements or
a controlled performance comparison. The public daemon retains a 2-GiB ceiling
and a 64-MiB redb cache. Its RSS is not whole-host memory or filesystem-cache usage.

Runtime source: `c5aa9727c268573ab7a2f274dc2fc103cf3c3a13`, binary SHA-256
`cfe864ff29154ce63981f1cfb28a81c53ee052b567403b78b54afc17b5f62128`.
Original snapshot build fingerprint:
`82f81e97b276a6374be83730a11a824195b75f77a9bbe968a96ef8e7cbf81c2b`.
Both remain distinct in generation/activation metadata.

Validation on this host:

- `cargo fmt --all -- --check`; strict all-target locked Clippy.
- `cargo test --locked --all-targets -- --test-threads=2`: **143 unit**, **18 integration**,
  **2 load-client** tests; one ignored subprocess helper is invoked by its parent.
  ARM64 release tests under QEMU pass the same counts with
  `QEMU_LD_PREFIX=/usr/aarch64-linux-gnu` and the cross linker/runner. This is not
  native ARM hardware/CI evidence.
- An initial default-concurrency run timed out in an existing authentication HTTP
  test while other jobs were running. The final full runs use two test threads;
  no test was removed or assertion weakened. This remains a load-sensitivity limit
  to investigate during release qualification. QEMU child execution initially
  lacked its sysroot environment; supplying it fixed the harness failure.
- Nine Python checks cover gateway isolation, bounds, snapshot admission,
  publication races, compatible resume, changed input rejection and activation
  fingerprint/count failures. They execute the normal release importer.
- The pinned official oracle passes **71 HTTP authorization**, **45 TCP/45 HTTP WHOIS**,
  existing organisation/policy/schema checks and **five NRTMv3 framing/omission**
  vectors. No new private Actions passing run is claimed.
- Locked 227-dependency RustSec audit passed against the local 1,403-advisory
  database. The new NRTMv3 fuzz smoke completed **431,580** executions in 31 seconds.
  Current native Debian/RPM packages built; packaging source is unchanged.
- Real Chromium public checks cover all six views at 320/390/1440 px, IP/prefix/
  origin/set queries, object summaries, live schema and queries, source details,
  ARIN's web exclusion, an empty truthful community directory and no third-party
  page requests. Dashboard HTML/CSS/JS force revalidation after updates.
- Public TCP WHOIS succeeds at both advertised IPs. IPv4/IPv6 HTTPS checks keep
  submission/admin/config/Git/private monitor paths at 404. External **Helsinki
  IPv4** and **Falkenstein IPv6** probes returned HTTP 200, real RADB AS3 content and
  trusted TLS 1.3: [IPv4 measurement](https://api.globalping.io/v1/measurements/2kRekcLOV3kMcHbnk00021R9P),
  [IPv6 measurement](https://api.globalping.io/v1/measurements/263R8noHxNyvoUwQ200021R9P).
  The Let's Encrypt certificate expires 1 February 2027; Caddy automatic renewal
  remains enabled. No DNS action is needed.

Local detailed logs are `/tmp/rirrd-sync-count-*`,
`/tmp/rirrd-staged-count-*`, `/tmp/rirrd-staged-arm64-final.log`,
`/tmp/rirrd-nrtm-complete-oracle.log`, `/tmp/rirrd-nrtm-gap-oracle.log`,
`/tmp/rirrd-nrtm-fuzz-final.log` and `/tmp/rirrd-nrtm-active-public-browser.log`.
Private generation/probe records are under `/var/lib/rirrd-dev/nrtm-release/`.
Secrets, real datasets and runtime records are not committed.
