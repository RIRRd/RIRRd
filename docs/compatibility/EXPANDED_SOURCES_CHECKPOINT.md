# Expanded sources and write classes — 3 November 2026

The live read-only test service remains https://rirrd.co.uk/dashboard/ and
`rirrd.co.uk:43`. The repository is private, main unchanged, submissions/admin
not public. This continuation does not establish full parity or production readiness.

## Real source coverage

A complete candidate verified **2,610,758 routing objects** and activated at
15:17:18 UTC. The source/class/permission/transport records are available through
`/dashboard/live.json`; no real snapshots/databases are stored in Git.

| Source | Accepted objects | Unsafe records withheld |
| --- | ---: | ---: |
| RIPE | 664,636 | 0 |
| ARIN | 211,232 | 0 |
| AFRINIC | 134,402 | 0 |
| LACNIC | 25,931 | 0 |
| BELL | 29,521 | 0 |
| ALTDB | 40,806 | 0 |
| TC | 72,245 | 14 |
| REACH | 18,028 | 0 |
| RADB | 1,365,912 | 32 |
| RIPE-NONAUTH | 44,468 | 0 |
| CANARIE | 1,888 | 0 |
| BBOI | 1,689 | 0 |

Coverage is routing classes, not every class in each registry. Existing duplicate
and malformed-lookup disclosures remain. ARIN is TCP-only, excluded from every
HTTP source scope. Five additional feeds have exact external actions: APNIC and
IDNIC require the APNIC bulk-access agreement; JPNIC eligibility/approval is
required for JPIRR; NTT asks for mirror onboarding contact; LEVEL3's published
snapshot/service addresses timed out from this VM. No agreements/messages were
sent for the owner. Official references are in the [deployment guide](../operations/LIVE_SERVICE.md).

Merit publicly publishes operational IRR data via anonymous FTP. RADB, ALTDB,
BELL, REACH and BBOI use those explicit endpoints; no failed HTTPS request is
silently downgraded. FTP is unauthenticated and its SHA-256 is an observation,
not publisher authentication. The dashboard discloses that provider/transport
scope and labels absent provider timestamps unreported. TC/CANARIE use HTTPS.
This data retains upstream terms, separate from the project's MIT code license.

## Failure, quarantine and resume

TC contained ASCII control characters inside member names. Strict import stopped
without changing the serving generation. The deployment now withholds the entire
unsafe record and counts it. It does not strip characters and invent another
member name. The pinned non-strict upstream parser rejects the three original
synthetic unsafe-member vectors used by the regression check. Invalid source
identity, credentials and other hard input failures still reject the candidate.

The resumed build verified its database and normalized-snapshot hashes and reused
six completed imports. It imported the remaining feeds, verified the full
2,610,758-object database, atomically switched `current`, restarted Rust and checked
the permitted HTTP source set. Public project pages remained available. The
candidate service had a two-CPU/2-GiB ceiling; its peak included charged file cache,
not a measured whole-host minimum. Resume rejects changed executable/catalog
inputs and the currently serving generation. Seven Python checks cover normal
routes, budgets, source/credential rejection, duplicate/Unicode preservation,
explicit transport policy, withheld records and simulated failure/resume using
the normal Rust importer. Per-source preparation hashes record mixed resume history.

The apparent database file is 4,295,503,872 bytes. This is not allocated-disk usage,
resident memory or a same-dataset upstream performance comparison. The existing
synthetic comparisons remain separately labeled; no new real-data throughput
claim is inferred from import diagnostics or live counters.

## Authoritative AS-block and domain profiles

Normal CLI and opt-in private HTTP now create, modify and delete AS-block/domain
objects. Both existing and proposed maintainers must authorize changes. Contact
references are source scoped; domains enforce distinct `zone-c` references and
prevent deleting a still-referenced zone contact. Tests cover incorrect proof,
missing/cross-source contacts, atomic batch rollback, TCP retrieval, mutation,
deletion and credential redaction. Domain attributes describing DNS are opaque
RPSL text, matching the pinned schema; the daemon performs no DNS delegation changes.

Pinned parser checks cover 16 common schema cases. The upstream domain parser
makes mnt-by optional; this bounded authenticated write profile requires it.
The HTTP comparison now passes 71 authorization decisions against official IRRd
4.5.4 at `8e8080dab744a348b7d0e6efcb53f403301dd075`. The original synthetic
WHOIS fixture has 12 objects; 45 TCP and 45 HTTP queries pass exact framing/content
comparison with the documented padding/auth-placeholder normalization. These do
not prove all response text, notification or account workflows match.

Local Linux and ARM64 QEMU release tests pass 139 unit, 17 integration and two
load-client tests. One ignored unit helper is invoked by the process-kill test.
Formatting, strict Clippy and the 227-dependency/1,403-advisory RustSec audit pass.
The parser fuzz smoke completes 177,454 executions in 31 seconds. Debian/RPM
packages build; an unprivileged, read-only-root/network-none container passes
query/UI/license and stop/reopen tests using the synthetic fixture. These are
local checks, not new authenticated CI results or native ARM qualification.

## Public verification and next gates

The real browser passes object, IPv4/IPv6 prefix, origin and AS-set flows and all
five views at 320/390/1440px without overflow or third-party requests. Each newly
loaded source returned a real HTTP object. Both advertised addresses returned
RADB AS3 over TCP WHOIS. RADB AS13335 has route-origin data without a matching
RADB aut-num object: its empty exact-object response is expected, and bgpq4
successfully generated 685 IPv4 and 270 IPv6 prefixes. BIRD/FRR syntax checks
use the generated candidate in isolated containers; no router is reconfigured.

The serving binary is built from `aa47c8c6377da957cdd1cebce8313abb19a84c16`, SHA-256
`6021b57155174457d2cee60385ca490d48e4ce2f094ae2576a1f5d791b6a9b39`.
Public assets/measurement IDs are recorded in the site branch's verification.
The AI disclosure, MIT notice and planned community directory remain visible.

Still open: the remaining inetnum/inet6num/irt/key-cert write profiles, OpenPGP,
scoped tokens, accounts, override/suspension/mail/notification parity; continuous
NRTMv4 publication/retention/recovery and bidirectional replication; wider queries,
live RPKI/scope/preference suppression; signed releases, migrations, sustained
load, independent security review and native architecture CI. New private Actions
results and repository settings remain unavailable through the deploy key.
