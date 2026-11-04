# UK node: public sources and embedded RPKI — 3 November 2026

> Historical checkpoint: embedded RPKI and RTR were removed in beta.2 on
> 4 November 2026. Validator measurements and commands below apply only to the
> recorded historical source revisions. They are not current features. See
> [the IRR-only checkpoint](IRR_ONLY_BETA2_CHECKPOINT.md).

The public node at **https://rirrd.co.uk/** now serves nineteen public IRR sources.
It is **RIRRd UK 01**, hosted in the **United Kingdom by HYEHOST LLC**, as supplied
by the project owner. No city, financial sponsorship or availability guarantee
is inferred. Earlier statements that APNIC/JPIRR/NTTCOM required owner enrollment,
or that ARIN had to be restricted to TCP port 43, were too broad and are superseded.
No enrollment or provider messages were sent. Published public feeds are used.

## Deployed evidence

- Runtime source: `d7929301ee4295b9c25b5b443e02930672c25cbb`.
- Runtime SHA-256: `04d606f13d443ca3a9b29d343d85dc900699efcde439adec878479a1d0048332`.
- Site content: `ca8735b` on `codex/coming-soon`; dashboard source `d792930`.
- Verified generation: `20261103T171000Z-public-feeds`, **4,993,140** objects at
  verification; **4,993,139** at `2026-11-03T17:57:19.701817+00:00` after live edits.
- Nineteen loaded sources: RIPE, RIPE-NONAUTH, ARIN, APNIC, AFRINIC, LACNIC,
  RADB, ALTDB, BELL, REACH, BBOI, TC, CANARIE, JPIRR, NTTCOM, IDNIC, LEVEL3,
  PANIX and NESTEGG. ARIN, IDNIC and LEVEL3 pass normal public GraphQL and
  key-browsing requests. There are no source-specific web exclusions.
- Seven feeds follow NRTMv3 every sixty seconds: LACNIC, BELL, ALTDB, REACH,
  NTTCOM, RADB and BBOI. Twelve use scheduled snapshots. This is not a claim
  that all nineteen have continuous incremental updates.

IDNIC and LEVEL3 use the public WOBCOM IRRd HTTPS GraphQL mirror. Snapshot queries
explicitly include valid/invalid/not-found RPKI states, all scope states and
visible/suppressed preference states. Peer serial and update marker must remain
stable across collection. Both received JSON and serialized gzip hashes are
recorded. IDNIC reported serial 205410; LEVEL3 reported 58752. Original primary
bootstrap endpoints failed from this VM; the working secondary feed is used.
JPIRR uses Merit's public snapshot after its NRTMv3 request returned access denied.
That transfer response does not block its snapshot from being mirrored. PANIX
and NESTEGG use public Merit snapshots. No source is left waiting for owner access.

Current class counts are maintained transactionally alongside writes and are
read from a small metadata index, rather than rescanning millions of records on
every dashboard poll. The observed counts were:

| Class | Objects |
| --- | ---: |
| as-set | 77,297 |
| aut-num | 100,264 |
| filter-set | 219 |
| inet-rtr | 262 |
| peering-set | 381 |
| route | 3,324,246 |
| route-set | 27,437 |
| route6 | 1,462,907 |
| rtr-set | 126 |

Only the nine declared routing classes are imported into this profile; contact,
maintainer, key-cert, allocation and organisation classes are excluded. Snapshot
receipts, excluded classes, malformed lookup counters, duplicate identities and
unsafe-record quarantine remain visible. Counts are real and can change with edits.

## One-process RPKI integration

The same `rirrd-live` process runs optional NLnet Labs **Routinator 0.15.3** as an
attributed Rust library, with rpki-rs RTR framing. This is not an independent
rewrite of its cryptographic implementation. The BSD 3-Clause notice is bundled
in project, Debian, RPM and Docker distributions. The public API serves origin
checks, while RTR listens privately on `127.0.0.1:3323`.

The deployed normal RTR reset stream contained **1,029,414** origin
payloads: **792,395 IPv4** and **237,019 IPv6**,
matching the status API. All five trust anchors produced valid publication points;
**101 publication points were rejected**.
RRDP-only collection and rejected repositories limit coverage. Rsync fallback,
live IRR ROV suppression and complete RPKI production qualification remain open.
A first private cold collection reached roughly 1.1 GiB peak cgroup memory; the
warm combined public process later sampled 449.2 MiB RSS.
These are observations, not a whole-VM minimum or a paired performance comparison.

HTTP checks return VALID for 193.0.0.0/21 / AS3333 and INVALID for AS64496.
Unavailable/expired datasets return HTTP 503, never a fabricated NOT_FOUND.
Synthetic tests cover reset/delta/no-data RTR framing and freshness guards.
The live reset stream matches the full validated payload count. Cache locking
prevents concurrent writers; daemon restarts revalidate persistent cached data.

## Verification and operations

- `cargo fmt --all -- --check`; strict locked all-target Clippy.
- Locked tests: **147 unit tests**, **19 normal CLI/HTTP/TCP integration tests**,
  **2 load-client tests** passed; one existing manual performance test is ignored.
  The later constant-memory migration change passes its targeted test and a
  real 100k legacy-database server startup/count check (2.29 seconds observed).
- Pinned official IRRd v4.5.4 at
  `8e8080dab744a348b7d0e6efcb53f403301dd075`: **45/45 TCP and 45/45 HTTP**
  differential cases pass. An initial candidate used an older fixture; reloading
  the current 12-object fixture fixed the four missing-class mismatches. The
  reference HTTP worker needed its normal Redis preload completion signal after
  restart. No compatibility allowance was added to hide those harness issues.
- **12 Python integration checks** cover public gateway limits, source admission,
  serial races, compatible catalog extension and atomic runtime rollback.
- Native Debian/RPM archives built with the validator notice. Clean Debian 13
  container installation, unprivileged startup, WHOIS and database verification
  passed. ARM64 release cross-compilation and QEMU `config check` passed; this
  is not native ARM hardware or new passing ARM CI evidence.
- Locked audit: **341 dependencies**, no known vulnerability found against the
  local 1,403-advisory database. `rustls-pemfile 2.2.0`, inherited through the
  embedded validator, has maintenance warning RUSTSEC-2025-0134. It is not ignored
  or described as a warning-free audit.
- Chromium portal and dashboard checks pass at 320/390/1440 px, covering real
  object/IP/prefix/origin/set queries, recursive ASNs, key browsing, source handling,
  directory, all six dashboard views, empty results and no third-party page requests.
- Public HTTP redirects to HTTPS (308) over both families. Apex A/AAAA match the
  intended VM addresses. TCP WHOIS passes at both public IPs. Private monitor,
  config, Git, administration and submission URLs remain 404.
- External Helsinki IPv4 HTTPS/WHOIS request: [measurement](https://api.globalping.io/v1/measurements/2zYExBvKraiS7q1w400021RB5).
  External Amsterdam IPv6 RPKI status: [measurement](https://api.globalping.io/v1/measurements/2sU3mTMBmoeBeJvhl00021RB6).
  External London IPv6 RPKI status: [measurement](https://api.globalping.io/v1/measurements/2lQg71hAYIfIIQrch00021RB6).
  These returned 200 and trusted TLS. An initial Falkenstein IPv6 attempt timed
  out during TLS; the two later independent IPv6 checks succeeded. No DNS or
  host routing change was needed. The Let's Encrypt certificate expires
  1 February 2027; Caddy automatic renewal remains active.
- Existing parser fuzz binaries (parser sources unchanged by this continuation)
  completed **417,469 RPSL** and **203,956 NRTMv3** executions in separate 16-second
  smoke runs, with no crash. Corpora ran in temporary memory storage outside Git.
- Publication filename/blob checks and Gitleaks 8.30.1 tree/history scans passed.
  Runtime datasets, captured responses, cache, private keys and credentials are
  outside Git. The repository remains private, and main is unchanged. Deploy-key
  access cannot report private Actions run status; no new passing CI run is claimed.

The public website now starts with the operational portal, current class totals,
AS-set/object-key browsing, query tools and RPKI status. The historical IRR-only
100k comparison remains at `/project/`, with exact measured commits/dataset and
an explicit statement that its resource numbers exclude RPKI validation.
Commands are at `/guides/api/`; the BIRD/FRR guide is at `/guides/routers/`.
MIT and AI-assisted-development disclosures remain visible.

Runtime paths/services:

- `/usr/local/libexec/rirrd-live`, `rirrd-live.service`.
- `/var/lib/rirrd-live/current/irrd.toml`, `registry.redb`, `sources.json`.
- Persistent validator cache `/var/lib/rirrd-live/rpki`.
- `/usr/local/lib/rirrd-live/`, `rirrd-live-gateway.service` on loopback 9281.
- `/var/www/rirrd-coming-soon`, `/etc/caddy/Caddyfile`, `caddy.service`.
- `rirrd-live-refresh.timer`: daily 03:30 UTC plus jitter; candidate is prepared
  and verified before an atomic generation switch. Runtime resource ceiling is
  3 GiB / three CPU cores; the optional IRR-only profile can use lower limits.

An initial in-place upgrade was rolled back when its 30-second health window
expired during legacy index creation. Copying the multi-gigabyte rollback database
then stalled in disk writeback throttling; public queries were unavailable roughly
17:20:38–17:27:08 UTC while static pages remained online. The complete previous
backup inode was restored atomically. Block writeback throttling was temporarily
disabled during recovery and restored to its original 2000 microseconds. No
persistent kernel/network tuning was added. The corrected release aggregates
legacy counters with constant memory and one metadata write per source/class.
The final deployment switches preverified database and executable together;
rollback restores pointers and binary without copying a serving database.
The previous complete generation is retained; small configuration/binary backups
are under `/var/backups/rirrd-live/integrated-20261103`, and website backups are
under `/var/backups/rirrd-site/`.

Useful local evidence: `/tmp/rirrd-integrated-final-tests.log`,
`/tmp/rirrd-integrated-oracle-{tcp,http}.log`, `/tmp/rirrd-public-catalog-final-tests.log`,
`/tmp/rirrd-nineteen-{node,dashboard}-browser.log`,
`/tmp/rirrd-nineteen-gitleaks-{tree,history}.log`, and derived records under
`/var/lib/rirrd-dev/nineteen-*`. Full real datasets and cache are not committed.

## Remaining parity work

Full production parity remains incomplete: OpenPGP/account/token/mail workflows,
remaining class/reference semantics, full NRTMv4 publication/rollover/recovery,
continuous updates for the twelve snapshot sources, live IRR policy filtering,
broader query/schema parity, long outage/restart/upgrade qualification, sustained
load, native ARM CI and full router/RTR interoperability remain open. No public
authoritative updates, account administration or mirroring/journal export is
exposed by this read-only node.
