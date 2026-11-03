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
