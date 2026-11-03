# VM deployment verification — 3 November 2026

Deployed website source: `aa29905`. Host inspection confirmed Debian 13 x86-64,
root/sudo access, and both intended IPv4 and IPv6 addresses and default routes.
Only SSH and the Codex loopback listener existed before setup; nftables was empty.

- Public apex A: `192.67.35.6`; AAAA: `2a0f:6280:1d:1940:e6d8:d666:4268:d4c8`.
  Google public resolver returned both with TTL 300. `www` returned NXDOMAIN.
  No DNS changes are needed.
- Local IPv4 and IPv6 HTTP requests returned 308 to HTTPS; HTTPS returned 200
  with the expected page, local stylesheet and security headers.
- Let's Encrypt YE1 certificate covers `rirrd.co.uk`, valid from
  2026-11-03 09:30:28 UTC to 2027-02-01 09:30:27 UTC. Certificate chain and
  hostname validation passed. Caddy automatic certificate management is enabled.
- External IPv4 HTTPS checks in Switzerland and Sweden returned 200:
  https://check-host.net/check-report/55455181kccd . One India probe timed out.
- External IPv6 HTTPS GET from Singapore returned 200, the expected full page,
  and `tls.authorized=true` (TLS 1.3):
  https://api.globalping.io/v1/measurements/264baeclosnBt0Ggx00021R3u .
- Distribution `caddy.service` is enabled. Configuration validated before reload.
  Caddy admin listens only on 127.0.0.1:2019. HTTP/3 is disabled.
- Host input policy permits established traffic, loopback, ICMP/ICMPv6 and TCP
  22/80/443. SSH service and its authorized keys were preserved. Registry ports
  are not public. Isolated development container forwarding is scoped separately.

Paths: `/etc/caddy/Caddyfile`, `/var/www/rirrd-coming-soon/{index.html,style.css}`,
private certificate state `/var/lib/caddy/`, timestamped config/content backups
`/var/backups/rirrd-site/`, firewall backup `/var/backups/rirrd-site/firewall/`.

No browser visual check or real renewal cycle has elapsed. External services
provide point-in-time evidence; the live certificate remains managed by Caddy.

Additional independent probes: HTTPS over IPv4 returned 200 with valid TLS from
Helsinki and Buffalo:
https://api.globalping.io/v1/measurements/2zDHDtmLXTlj9Rqu900021R3v .
HTTP over IPv6 from Newcastle returned 308 with Location `https://rirrd.co.uk/`:
https://api.globalping.io/v1/measurements/2EaCWlWfhBAhKyUF600021R3w .
