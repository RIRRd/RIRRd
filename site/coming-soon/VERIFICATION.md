## Current live portal — 3 November 2026

The live site at https://rirrd.co.uk/ serves the UK/HYEHOST LLC node portal.
Nineteen public IRR sources are loaded (4,993,140 verified RPSL objects), with
seven NRTMv3 feeds and twelve scheduled snapshots. ARIN is included in public
WHOIS, GraphQL and browsing. Live class counts, AS-set/key browsing, API commands,
and embedded RPKI status/origin checks are available. The same Rust process
validated 1,029,414 VRPs in the deployed check; 101 rejected publication points
and RRDP-only coverage are disclosed. RTR remains loopback-only.

Runtime source is d792930; site content is ca8735b. Chromium portal and dashboard
checks pass at 320/390/1440 px. HTTP redirects, trusted TLS and real queries pass
from external IPv4 (Helsinki) and IPv6 (Amsterdam/London) probes. One initial
Falkenstein IPv6 handshake timed out; later IPv6 probes succeeded. Caddy validates
and automatic renewal remains enabled. Current reusable files and full evidence
are in [the checkpoint](../../docs/compatibility/UK_NODE_PUBLIC_SOURCES_RPKI.md).

The benchmark page is now `/project/`; the primary URL is the operational portal.
No new throughput comparison is claimed for the integrated validator: the existing
100k table is explicitly scoped to its recorded IRR-only commits and dataset.

The earlier verification notes below are historical checkpoints.

## Continuous mirror console — 3 November 2026, 16:12 UTC

The published dashboard uses core UI revision
`c684c3fb169067560234f50078238cf72968bc12`; the separately deployed daemon remains at
`c5aa9727c268573ab7a2f274dc2fc103cf3c3a13`. Source details precede the status table, and navigation
returns to the page heading so the class breakdown is immediately discoverable.
The source dataset activated at 16:10:22 UTC. RADB, ALTDB, BELL, REACH and BBOI
passed two automatic polls 60 seconds apart with committed cursors. Seven other
loaded feeds remain daily snapshots, and five tracked feeds remain pending.
The aggregate committed count reached 2,611,104 at the recorded second poll;
per-source class counts remain explicitly the bootstrap counts.

The six console views pass real Chromium checks at 320/390/1440 px, including
IP covering routes, object summaries, live GraphQL schema/query execution and
per-source class/publication details, without third-party page requests. Source
and artifact labels disclose NRTMv3/FTP transport limits and cache accounting.
Dashboard static assets force revalidation. Contact/sponsor/location details
remain unreported pending operator input.

External verified RADB AS3 requests returned trusted HTTPS 200 from Helsinki
IPv4 and Falkenstein IPv6:
[IPv4](https://api.globalping.io/v1/measurements/2kRekcLOV3kMcHbnk00021R9P),
[IPv6](https://api.globalping.io/v1/measurements/263R8noHxNyvoUwQ200021R9P).
TCP WHOIS succeeds at both advertised addresses. Public admin/submission,
configuration, Git and private native-monitor paths remain 404. TLS expires
1 February 2027 and Caddy automatic renewal is enabled. No DNS action is needed.

Current local evidence is 143 unit/18 integration/two client tests on Linux and
ARM64 QEMU, nine deployment checks, 71 upstream authorization and 45 cases for
each WHOIS transport, plus five official NRTMv3 omission/framing vectors.
No new passing private Actions run or full parity is claimed. The detailed core
checkpoint is `docs/compatibility/CONTINUOUS_MIRROR_CHECKPOINT.md` on
`codex/linux-parity`. The benchmark charts retain their original separate
synthetic dataset and commits; live counts are not a new upstream benchmark.

The sections below are earlier deployment evidence.

# Expanded live verification — 3 November 2026, 15:20 UTC

Current static-content commit: `9b946f0bbeee938e394fb1e084bbfd7dac73e451`.
Current HTML SHA-256: `0638d2ea112571bcb10d3ec5538ad4d5e8ce5ad00a6a4f1885d87857c9f57854`.

The current service contains **2,610,758 routing objects across twelve sources**:
RIPE, RIPE-NONAUTH, ARIN, AFRINIC, LACNIC, RADB, ALTDB, BELL, REACH, BBOI, TC and
CANARIE. The previous five-source evidence below is historical. Five feeds remain
pending: APNIC, IDNIC, JPIRR, NTTCOM and LEVEL3, with their exact access/connectivity
actions listed on the dashboard and in the core source catalog.

The expanded candidate stopped on unsafe TC member records without changing the
live generation. It resumed six completed imports, withheld/reported 14 unsafe TC
records and 32 RADB records, verified every index/count, and activated successfully
at 15:17:18 UTC. The serving executable was subsequently updated to the verified
15-profile build. Public submission/admin routes remain disabled. Source hashes
are recorded; FTP relay data is explicitly labeled unauthenticated rather than
presented as authenticated or near-real-time. Missing provider timestamps remain
unreported. ARIN remains TCP-only and excluded from all HTTP queries.

Current code evidence: 139 unit/17 integration/two client tests, same release tests
under ARM64 QEMU, formatting/strict Clippy/RustSec pass; 71 pinned authoritative
HTTP decisions, 45 TCP and 45 HTTP WHOIS cases, 16 new AS-block/domain schema cases.
New class tests include maintainer proof, zone-contact references, deletion guards,
source isolation, rollback and redaction. Full parity remains incomplete.

The updated public browser passed all five views at 320/390/1440px and normal
object/prefix/origin/set queries without third-party requests or JS exceptions.
Every new source returned a real object. TCP queries to RADB AS3 passed both
advertised addresses. External trusted-TLS HTTP WHOIS probes returned AS3/RADB:

- [Buffalo IPv4](https://api.globalping.io/v1/measurements/2GSmK1tFe8aLzffCE00021R8a)
- [Singapore IPv6](https://api.globalping.io/v1/measurements/2BGE4qjs1Bv83oBVu00021R8a)

Earlier exact-object probes for RADB AS13335 correctly returned 204: that source
has routes originated by AS13335 but no matching aut-num object. Its actual origin
expansion generated 685 IPv4 and 270 IPv6 prefixes through bgpq4; BIRD and FRR
accepted the complete generated candidates in isolated parser containers. No
router was reconfigured. Benchmarks remain separately scoped synthetic results.

MIT/AI disclosures, TLS maintenance, services, firewall and private runtime paths
are unchanged from the deployment details below. No DNS action remains. Both the
source repository and main remain private/unchanged respectively; new Actions
results remain unavailable through the deploy key.

---

# Live deployment verification — 3 November 2026

Public URL: https://rirrd.co.uk/ · [Live dashboard](https://rirrd.co.uk/dashboard/)
· [Router guide](https://rirrd.co.uk/guides/routers/).
Static content commit: `b3fa22006206805f8faa016ee30ec0ef0f5f451c` on `codex/coming-soon`.
HTML SHA-256: `e32b84930cc8f08b5279614fa563a66814890ee0e46f30d8da83f454d2c3594a`.

The owner authorized public read-only registry testing after the initial static-only
site. TCP WHOIS/bgpq4 is now available at `rirrd.co.uk:43`. The dashboard uses this
RIRRd instance's real routing data and measured telemetry; it is no longer the
synthetic demo. The comparison page retains its separate synthetic methodology.

- DNS A is `192.67.35.6`; AAAA is
  `2a0f:6280:1d:1940:e6d8:d666:4268:d4c8`. Public IPv6 resolver checks pass.
  `www` remains NXDOMAIN and no redirect was added. No DNS change remains.
- Origin IPv4 and IPv6 HTTPS responses validate normally. HTML, CSS, comparison
  and small-host JSON, dashboard assets, router guide and MIT notice match source.
  HTTP redirects with 308 to HTTPS. The trusted Let's Encrypt certificate expires
  1 February 2027; automatic Caddy certificate maintenance/renewal remains enabled.
- Caddy configuration was validated before deployment and reloading. The
  distribution `caddy.service` remains active and enabled. Its public file root is
  `/var/www/rirrd-coming-soon`; configuration is `/etc/caddy/Caddyfile`.
- Only `/graphql/`, `/v1/whois/`, `/dashboard/meta.json` and `/dashboard/live.json`
  reach the read-only loopback gateway. `/v1/submit/`, `/admin/`, `/.git/config`
  and `/irrd.toml` return 404 over both families. Forged client-address headers
  are overwritten by Caddy. Explicit ARIN HTTP queries are denied; default
  GraphQL status also excludes ARIN. ARIN remains available over TCP WHOIS.
- Real browser checks pass object, IPv4/IPv6 prefix, origin and AS-set queries,
  no-result handling and source selection. All five views fit 320/390/1440px.
  Actual-page screenshots were inspected. No third-party page requests or
  JavaScript exceptions were observed. AI disclosure remains visible on mobile.
- WHOIS responds on both advertised addresses. bgpq4 generated real RIPE AS3333
  candidates containing six IPv4 and one IPv6 prefixes; BIRD 2.17.5 and FRR 10.3
  parsed the complete candidate in isolated containers. No router was reconfigured.
- The live dataset contains 1,080,669 routing objects in RIPE, RIPE-NONAUTH, ARIN,
  AFRINIC and LACNIC. Twelve other feeds have explicit pending access/transport
  actions. This is daily snapshot coverage, not near-real-time replication or
  full IRRd parity. Source terms are separate from the project's MIT license.
- `rirrd-live.service` and `rirrd-live-gateway.service` are enabled, unprivileged
  and resource bounded. Rust HTTP is loopback 9280; gateway is loopback 9281.
  `rirrd-live-refresh.timer` is enabled for daily 03:30 UTC plus up to 30 minutes
  jitter. A second complete refresh verified all objects, switched generations,
  restarted Rust and passed HTTP health checks at 14:44 UTC while Caddy served.
- Source/runtime separation: Rust binary `/usr/local/libexec/rirrd-live`, gateway
  and refresh tools `/usr/local/lib/rirrd-live/`, private data/current generation
  `/var/lib/rirrd-live/current`. No snapshots, databases, host keys or credentials
  are installed under the public root or committed to Git.
- Firewall input allows TCP 22/43/80/443, established/loopback and ICMP/ICMPv6.
  SSH remains active. Temporary router/scale firewall exceptions were removed;
  oracle, browser preview and private synthetic evaluation services are stopped.

Last site/config backup: `/var/backups/rirrd-site/20261103T144039Z/`.
Pre-WHOIS firewall backup: `/var/backups/rirrd-live/20261103T1437/nftables.conf`.

## External evidence

Globalping dashboard HTTPS probes in São Paulo (IPv4) and Los Angeles (IPv6)
returned 200 with trusted TLS. London probes returned actual AS3333 WHOIS bodies
through HTTPS in both families, with trusted TLS. TCP port 43 was reached in both
families: the IPv4 sample received two of three probes; IPv6 received three.
A first Lagos IPv4 HTTP probe timed out; a London recheck returned 308. IPv6 HTTP
returned 308. An initial WHOIS probe encoded the query into the path and got 404;
corrected probes use the API's separate query-string field. These initial failures
are retained in private reports, not relabeled as successful server checks.

Measurement IDs (provider retention applies):

- [Dashboard IPv4](https://api.globalping.io/v1/measurements/2oINSYk4j7q6QxbND00021R7u)
- [Dashboard IPv6](https://api.globalping.io/v1/measurements/2kuGB2k7CFLWH2RPD00021R7u)
- [Real WHOIS HTTPS IPv4](https://api.globalping.io/v1/measurements/2IHp4murxNQEaOWrE00021R7v)
- [Real WHOIS HTTPS IPv6](https://api.globalping.io/v1/measurements/2pRBzZu1tqEXeyT8k00021R7v)
- [TCP 43 IPv4](https://api.globalping.io/v1/measurements/2Q42bdt61mYQZQUBl00021R7v)
- [TCP 43 IPv6](https://api.globalping.io/v1/measurements/2KE8ju2rN4sNub0SF00021R7v)
- [HTTP IPv4 recheck](https://api.globalping.io/v1/measurements/2r2gVLOyojtFkcQ3p00021R7w)
- [HTTP IPv6](https://api.globalping.io/v1/measurements/28DC9I2dfD2pKLLdZ00021R7u)

The probe provider can truncate recorded bodies. Full origin assets were separately
hash-checked. These probes establish only their individual observations, not a
sustained availability or production-capacity guarantee.

The source repository remains private, main unchanged. Full RIRRd parity and
production qualification remain incomplete. New private GitHub Actions evidence
is unavailable with this session's deploy-key access.
