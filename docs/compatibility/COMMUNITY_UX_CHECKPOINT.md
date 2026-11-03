# Community website and independent console — 3 November 2026

Branch: `codex/community-ux`, based on `efe2694`. UI implementation: `c494377`,
community deployment: `23dcb96`, telemetry isolation: `7059349`.
Main and repository visibility are unchanged. Full IRRd parity remains incomplete.

## Products and deployed paths

- Community website: https://rirrd.co.uk/; reviewed node discovery, UK 01 hosted
  by HYEHOST LLC, distinct hosting and financial sponsorship fields, self-host
  information and private node-submission form. Additional nodes are not invented.
- Public query console: https://rirrd.co.uk/dashboard/; seven task-focused views,
  live counts, per-source modes/serials, bounded class/key browsing and RPKI state.
- Independent console: `web/dashboard/`, bundled into the Rust executable, without
  community listings, sponsor directory, submission service or community account.
- Separate community code/deployment: `site/community/`. Atomic static releases
  under `/var/www/rirrd-community-releases/`; `/var/www/rirrd-community` points at
  the active release. Caddy serves these and narrowly proxies existing query APIs.
- Private review queue: `rirrd-community.service`, loopback 9282, unprivileged
  account, 64 MiB / 20% CPU ceilings; `/var/lib/rirrd-community/` mode 0700,
  SQLite/rate key mode 0600. Private contacts never enter Git or the public tree.
  Retention cleanup runs hourly for records older than 90 days. Approval requires
  independent review and a subsequent reviewed directory commit/deployment.

The form has bounded fields/body/concurrency, origin/framing checks, rate limits,
operator attestation and a pending cap. It never fetches submitted endpoints,
auto-publishes nodes or sends email. Successful UI submissions were tested only
against a temporary isolated queue; no dummy node was submitted to production.
The real endpoint rejects an empty payload with 422. Queue/admin/script/config,
`.git` and environment paths return 404 publicly.

## Queries and scope

Prominent presets and six copyable command cards cover ASN IPv4 (`!g`), IPv6
(`!6`), recursive AS-set membership (`!iSET,1`), set IPv4/IPv6 (`!a4`/`!a6`), object
lookup and prefix lookup modes. Errors and empty answers are distinguished.
Source selection is separate from query syntax. API and BIRD/FRR examples are
linked at https://rirrd.co.uk/guides/api/ and https://rirrd.co.uk/guides/routers/.
Public examples use the public hostname. A private console accurately describes
its configured local listener; public RPKI status omits the internal RTR bind.

At verification: 4,993,322 RPSL objects across nineteen sources, seven sixty-second
NRTMv3 feeds and twelve daily snapshot feeds. Counts change as feeds advance.
The mirror profile retains routing classes, rather than every contact/account
class; a raw object count against another service is not a coverage-parity test.
All loaded sources including ARIN are available through the public read interfaces.
Embedded RPKI recovered after the native UI restart: 1,029,451 VRPs, all five trust
anchors, 100 rejected publication points disclosed. RRDP-only coverage limits,
private RTR and unfinished IRR policy suppression remain explicit.

The performance page retains the existing bounded 100,000-object synthetic
comparison with scope and raw evidence. No new throughput, minimum-host size or
speedup claim was inferred from a UI refresh or a different live dataset.

## Resource and deployment observations

The 62 GiB VM had about 58 GiB available and zero swap usage during final checks;
roughly 36 GiB of buff/cache is substantially reclaimable Linux cache, not the
Rust process RSS. Cache, process RSS, cgroup accounting and CPU as a percentage
of one core are now separately labeled. Validation/import bursts and compilation
are distinct from idle behavior. These observations are not a small-host sizing
qualification. No cache flush, database reimport or global kernel tuning was used.

Gateway telemetry interval increased from five to fifteen seconds, immutable
bootstrap receipts are cached by generation/mtime, and PID discovery uses cgroup
membership instead of spawning `systemctl` on each sample. The browser polls
visible relevant views without overlapping requests. No frontend framework,
analytics or third-party scripts/assets were introduced.

The website deploy validated Caddy first and backed up configuration/site files.
Native UI build `70593493597181b534401af063974829df56bbe5` has SHA-256
`16fb3b1414924f7e232072ce3ba7caae5e4cbae58aaeeaf0d368ac52d62e3121`.
Rust source and Cargo manifests were unchanged from the previous verified live
backend. A small binary/metadata backup was taken before an atomic executable
switch, preserving the same database generation; the native restart and nineteen-
source readiness check completed in 2.11 seconds. No multi-gigabyte database copy
was placed in the serving outage. The static website remained available.

## Verification

Local commands passed:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 -m unittest discover -s tools/live-service -p 'test_*.py'
python3 -m unittest discover -s site/community -p 'test_*.py'
python3 tools/check_dashboard.py --url http://127.0.0.1:19480/ui/ --mode live
python3 tools/check_community_ui.py --url https://rirrd.co.uk/
python3 tools/check_public_tree.py --include-untracked
```

Results: 147 unit tests pass with one existing performance helper ignored,
19 CLI/HTTP/TCP integration tests, two load-client tests, thirteen gateway tests
and four submission tests. The locked RustSec audit reports no known vulnerable
crate and retains the disclosed unmaintained `rustls-pemfile` warning. Chromium
checks cover all seven views at 320/390/1440 px, real prefix/set/member queries,
source filtering/browsing, schema/commands, redaction and RPKI VALID. Private
console checks verify absence of community listings/hosting data. Temporary
preview and synthetic console servers were stopped after verification.

Gitleaks 8.30.1 offline scans of all reachable history and the exact publication
candidate report no leaks under the existing narrowly reviewed configuration.
Filename/blob checks find no runtime credentials, private keys, databases or
oversized artifacts. Logs, screenshots, queue state and scan reports remain
outside Git under `/tmp/rirrd-community-*` and `/var/lib/rirrd-dev/`.

Public A/AAAA equal `192.67.35.6` and `2a0f:6280:1d:1940:e6d8:d666:4268:d4c8`.
Host IPv4/IPv6 checks return HTTPS 200 and HTTP 308 to HTTPS; certificate chain and
hostname verify, with Let's Encrypt certificate expiry 1 February 2027 and Caddy
automatic renewal. Globalping external HTTPS probes return 200 with matching
content from Sydney/Buffalo over IPv4 and Lagos over IPv6. Helsinki IPv6 fails;
reachability is therefore confirmed from working probes, not universally claimed.
Measurement IDs: `2XEX4rmZ4NSDeBm4500021RBi` (IPv4),
`2kMwtywHsHSbB2INr00021RBi` (IPv6). No DNS action is required for the working site.
No new public firewall ports or registry write/admin endpoints were opened.

Private GitHub Actions read access remains unavailable to the deploy key, so no
new passing CI link is claimed. Existing oracle/fuzz/packaging evidence remains
scoped to its tested commits. This frontend change does not establish additional
authentication/account/class parity, bidirectional/NRTMv4 recovery, live policy
filtering, broader query parity or release qualification.
