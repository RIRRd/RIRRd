# Live experimental registry — 3 November 2026

The owner explicitly authorized public live read-only service after the earlier
static-site restriction. The repository remains private. Main is unchanged;
development and deployment work is on separate `codex/` branches.

## Deployed behavior

- https://rirrd.co.uk/dashboard/ is a live read-only registry console, not the
  previous synthetic demo. Its normal object, exact-prefix, origin and AS-set
  queries execute against this Rust service. No queries are forwarded to another
  registry. The 15-second visible-page refresh displays measured process/cgroup
  memory, CPU, uptime, object count, apparent database size, current TCP WHOIS
  connections, HTTP attempts/errors and recent gateway p95 latency.
- `rirrd.co.uk:43` accepts public IPv4/IPv6 WHOIS and bgpq4. The HTTPS gateway
  exposes only read-only WHOIS GET, GraphQL POST and public metadata/telemetry.
  Authoritative submission, account/admin and journal/mirroring publication are
  disabled/private. Direct development ports are not public.
- The project site retains the measured synthetic comparison, separate from
  live counters, plus the small-host memory/time trade-off and per-file cold
  cache recheck. MIT and explicit AI authorship/contribution guidance are visible.

The verified routing-only dataset has **1,080,669 objects**:

| Source | Objects | Coverage / observations |
| --- | ---: | --- |
| RIPE | 664,636 | Nine published routing classes; 133 malformed optional lookup values retained in text but not indexed |
| ARIN | 211,232 | TCP WHOIS only; 49 duplicate canonical identities collapsed using the final record |
| AFRINIC | 134,402 | Routing-class subset; three malformed optional lookup values not indexed |
| LACNIC | 25,931 | Routing-class subset |
| RIPE-NONAUTH | 44,468 | Published aut-num, as-set, route and route6 splits |

No contact/maintainer/key-cert/address-allocation classes were imported. Source
data retains upstream terms rather than inheriting the project's MIT license.
ARIN is excluded from every HTTP query, including default queries and explicit
source arguments. Its object data is distributed here only through TCP WHOIS.
The [source catalog](../../tools/live-service/sources.json) and public status
list all 17 tracked feeds, with 12 pending rather than fictitious coverage.

APNIC needs the owner's signed bulk-access AUP; NTT asks prospective mirrors to
contact its administrator; JPIRR needs mirror-policy/onboarding review. RADB's
FTP listing is reachable but downstream redistribution/transport configuration
is pending. ALTDB's official HTTPS name fails certificate matching, LEVEL3 times
out and IDNIC fails TLS negotiation from this VM. BELL, REACH, TC, CANARIE and
BBOI still need completed permission/transport review. See the
[deployment guide](../operations/LIVE_SERVICE.md) for official references and
actions. No messages or agreements were sent/signed on the owner's behalf.

## Real snapshot defects and bounded handling

Published RIPE filter-set policies include a 2.17 MB single-line record. Snapshot
admission and lookup extraction now accept at most 4 MiB per object; normal
submission admission remains 64 KiB. CLI import, integrity, restart and HTTP
source-isolation regression tests cover the distinction. NRTM's separate object
bounds and full large-object replication parity remain open.

ARIN contains case-equivalent duplicate route origins and U+2028 within descriptions.
The deployment normalizer uses bounded disk-backed identity tracking, preserves
Unicode paragraph separators and keeps the final canonical duplicate. This follows
the pinned upstream snapshot `upsert_rpsl_object` behavior; it does not weaken
normal CLI duplicate rejection. Synthetic tests cover case/leading-zero identity
equivalence, final-record preservation, Unicode, source mismatch and credential
admission failure. This is not a complete real-data differential comparison.

## Verification

Native Linux formatting, strict Clippy and locked all-target tests pass: 138 unit,
16 integration and two load-client tests. One ignored unit helper is invoked by
the process-kill integration. ARM64 release tests under QEMU pass the same set;
this is not native ARM CI. RustSec checks 227 locked dependencies against 1,403
advisories. The expanded snapshot-parser fuzz smoke completes 221,757 executions
in 31 seconds. Native Debian/RPM packages build with MIT notices. The container
image passes unprivileged, read-only-root, network-none runtime/query/license and
graceful-stop/reopen checks under one CPU and 128 MiB using the small synthetic
fixture; that budget is not a minimum for the real public dataset.

The pinned IRRd 4.5.4 oracle at
`8e8080dab744a348b7d0e6efcb53f403301dd075` still matches:

- 39 TCP and 39 HTTP WHOIS cases, exact framing included.
- 47 authoritative HTTP authorization decisions.
- Ten organisation / 25 policy/router schema cases, nine organisation fields and
  32 policy/router field types plus the documented value cases.
- Twelve real bgpq4 client profiles on the original synthetic integration fixture.

Live public browser checks pass object, IPv4/IPv6 exact-prefix, origin and populated
AS-set queries, empty results, source selection and ARIN exclusion. All five views
fit 320/390/1440 px without page overflow; no third-party requests or JavaScript
exceptions were observed. Actual public-source AS3333 lookup examples were checked
against loaded data rather than assuming the contents of an arbitrary AS-set.

The real public endpoint generated a candidate with six IPv4 prefixes and one
IPv6 prefix for RIPE AS3333. BIRD 2.17.5 and FRR 10.3 accepted the full candidate
configuration in isolated parser containers. No router configuration was applied.

The first complete generation was verified and activated at 14:37 UTC. A second
full refresh built while it served, verified all 1,080,669 objects, atomically
switched generations, restarted Rust and passed its normal permitted-source HTTP
health check. The job completed successfully at **14:44:00 UTC**, retained the
previous generation and left Caddy serving throughout. The next daily timer is
scheduled for 4 November between 03:30 and 04:00 UTC. Failure-triggered rollback
is implemented; a deliberate failure during this live switch was not injected.

The serving Rust binary was built from `ae369a6d440465956bb2f3770cfd75339e07c94f`,
SHA-256 `d4220ddd4189f9a9561b480fee6fb05dc39163f0443cb10760839d8de450beb3`.
Runtime release metadata and snapshot import-binary hashes are independently
reported. Public dashboard assets and Python controllers have later focused
commits; their exact versions are recorded in Git/deployment evidence.

External Globalping probes verified trusted HTTPS dashboard responses over IPv4
and IPv6, and real AS3333 HTTPS WHOIS bodies in both families. TCP-43 probes reached
both addresses (the IPv4 three-packet sample had one missed probe; IPv6 received
three). HTTP redirects passed both families on successful probes; an initial
Lagos IPv4 probe timed out and a London recheck returned 308. Origin IPv4/IPv6
checks verified full static-file hashes, certificate/expiry, redirects and private
route denials. See [site verification](../../site/coming-soon/VERIFICATION.md)
on the integrated branch for measurement IDs and deployment paths.

## Remaining limits

Snapshots refresh daily; they are not continuous near-real-time mirroring. There
is no new same-dataset upstream performance comparison for this larger real
dataset. Live process/cgroup counters do not establish whole-machine memory
minimums: cache ownership/sharing affects their scopes. The verified small-host
benchmark remains a separately bounded synthetic experiment.

Full authentication/accounts/class coverage, NRTMv4 publication/scheduling and
bidirectional recovery, live RPKI/scope/preference semantics, broader queries and
release/security/sustained-load qualification remain open. Newly configured
private Actions runs and repository protection settings cannot be independently
verified with the available deploy-key access. Local results and old CI links do
not establish a new passing CI run or production readiness.
