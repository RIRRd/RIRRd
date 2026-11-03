# Deployment verification — 3 November 2026

Public URL: https://rirrd.co.uk/
Static content commit: `9d3a172` on `codex/coming-soon`.
HTML SHA-256: `d1b49ebe525edad9c1c60682318af2c24994be0156c4fcf513d424025c778023`.

- Apex public DNS A: `192.67.35.6`; AAAA:
  `2a0f:6280:1d:1940:e6d8:d666:4268:d4c8`. Queried Cloudflare's public
  IPv6 resolver. `www` is NXDOMAIN for A and AAAA; no redirect was added.
- Origin IPv4 and IPv6: HTTPS 200, normal certificate-chain validation, full HTML
  matches the committed source byte-for-byte. HTTP 308 redirects to HTTPS.
  `/graphql/` returns 404; the public server is static.
- External HTTPS: Globalping Tokyo IPv4 and Los Angeles IPv6 probes resolved
  the intended addresses, returned 200 and verified TLS 1.3 / Let's Encrypt.
  Both returned the new measurement content. The probe service truncates its
  recorded body; full origin content was separately hash-checked.
- External HTTP: Buffalo IPv4 and Newcastle IPv6 probes returned 308 with
  `Location: https://rirrd.co.uk/`.
- Certificate expires 1 February 2027; automatic Caddy TLS management and renewal
  remain enabled. The distribution `caddy.service` is active. Configuration was
  validated before deployment and before reloading.
- Chromium device emulation: 320px, 390px and 1440px layout checks passed without
  horizontal page overflow. Tables scroll within their labeled regions. Desktop
  and phone screenshots were inspected. No scripts, forms or remote page assets.
- `style.css` and `comparison.json` responses match source. Deployment files and
  Git metadata are not served. Root contains only static public files.
- Input firewall still permits TCP 22/80/443, loopback, established traffic and
  ICMP/ICMPv6. SSH remains active. Benchmark/oracle services are stopped and the
  temporary scale-bridge forwarding rule was removed. No public registry opened.

The deployed files are `/var/www/rirrd-coming-soon/{index.html,style.css,comparison.json}`.
Caddy configuration: `/etc/caddy/Caddyfile`.
Pre-deployment backup: `/var/backups/rirrd-site/20261103T122523Z/`.

External measurement IDs (provider retention applies):

- [HTTPS IPv4](https://api.globalping.io/v1/measurements/2r31pwAGzCaIEhi4i00021R5m)
- [HTTPS IPv6](https://api.globalping.io/v1/measurements/218o4oRKFJb8q78P200021R5m)
- [HTTP IPv4](https://api.globalping.io/v1/measurements/2PL7zsiRtBmKupKU400021R5n)
- [HTTP IPv6](https://api.globalping.io/v1/measurements/26hzpYh8re0pll03400021R5n)

No DNS action remains. The repository remains private. Full RIRRd parity and
production qualification remain incomplete; these site checks do not establish
registry readiness.
