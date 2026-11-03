# Two-domain beta launch — 3 November 2026

Version: **0.1.0-beta.1**. Working branch: `release/v0.1.0-beta.1`.
Full IRRd parity and production qualification remain incomplete.

## Public products

- **https://rirrd.co.uk/**: project showcase, local vector identity, recorded
  comparison graphs, self-hosting/API/router/RPKI documentation and beta status.
- **https://irrd.tools/**: separate community identity, public UK node directory,
  reviewed private submissions, live queries and source/activity graphs.
- **https://irrd.tools/dashboard/**: independent console adapted to the public
  node, with source, HTTP rate/p95, RSS/CPU and trust-anchor/VRP graphs.
- The daemon's `/ui/` remains independently self-hosted. It does not require a
  community account or include the public directory/submission service.

The first node remains UK 01, hosted in the United Kingdom by HYEHOST LLC.
Hosting credit and financial sponsorship are separate fields. Extra nodes, cities,
uptime and sponsors are not fabricated. MIT licensing and AI-assisted authorship
are disclosed; human and AI-assisted contributions are welcome with review/tests.

The project and community static releases are separate symlinks at
`/var/www/rirrd-project` and `/var/www/irrd-tools`. Source/deployment lives under
`site/project`, `site/community`, `site/brand`, `site/Caddyfile`, `site/deploy.sh`.
`caddy`, `rirrd-live`, `rirrd-live-gateway` and `rirrd-community` are active.
Small configs/binaries and prior release pointers were backed up under
`/var/backups/rirrd-web`; no serving database copy or reimport was needed.

## Runtime and RPKI changes

Native source: `ce4d24fc005e1ae507999fc9790a63b44eb157e7`.
Executable SHA-256: `a3b020fa16f77fd303e78b9790903f57c8727a70ed1522a177f0a3b4f632ce5d`.
Later release tooling/documentation commits do not change its Rust/Cargo/console
inputs. The final native switch recovered nineteen-source readiness in 1.78 s;
static sites remained available. The database format and active generation were
preserved. The source catalog still has seven sixty-second NRTMv3 feeds and twelve
daily snapshots, not nineteen continuous feeds.

The optional in-process validator continues to use attributed Routinator 0.15.3 /
rpki-rs. New behavior:

- Up to 32 most-specific covering ROAs explain a GET origin verdict. An explicit
  truncation flag does not limit the complete validation decision.
- POST `/v1/rpki/validate` accepts 1–100 routes in a body bounded to 16 KiB, pins one
  immutable dataset and returns a single serial/timestamp. Invalid input fails the
  whole batch; unavailable/expired data remains 503, never NOT_FOUND.
- IPv4/IPv6 totals, validation duration/age and finalized per-anchor contribution
  and rejection counts are exposed. Contribution counters are read after upstream
  global deduplication/filtering, rather than before snapshot finalization.
- Verified prefixes build the ROV index directly from binary address/length
  values, avoiding per-payload string formatting/parsing. No new whole-validator
  speedup or minimum-host claim is inferred from this code change.
- An incomplete cached quick pass immediately falls back to full validation,
  without the ordinary sixty-second failure retry delay. Publication still waits
  for actual validation. One final warm pass completed in 39.44 s.

Public verification observed **1,029,480 VRPs**: **792,451 IPv4**, **237,029 IPv6**,
all five trust anchors and **98 rejected publication points**. Per-anchor family
contributions sum to the published family totals. Counts change with refreshes.
RRDP-only collection, excluded rsync-only coverage, private loopback RTR and
unfinished live IRR policy suppression remain explicit beta limits.

## Monitoring scope

Gateway history keeps up to 1,440 one-minute real aggregate samples for 24 hours
in `/var/lib/rirrd-telemetry/history.json` (0600, parent StateDirectory 0700).
It contains no visitor addresses, queries, credentials or node-submission contacts.
Counter resets produce an unavailable first rate, not a negative value. Missing
samples remain gaps. Latency samples use a bounded interval buffer, so idle periods
do not repeatedly plot an old query as a new timing observation. Counters cover
HTTP gateway attempts, not TCP traffic or a capacity benchmark.

SVG charts are local, keyboard-inspectable and labeled with observed values.
No chart history is generated to fill a past day. Project performance graphs use
the existing historical 100,000-object synthetic IRR-only comparison and its exact
commits, with RPKI disabled. They are separate from current live-node monitoring.

After stopping temporary preview/console/oracle workloads, the 62 GiB VM had
about 59 GiB available, zero swap usage and 194 GiB disk available. Linux cache
is separately explained in the console; these observations do not qualify a
minimum combined IRR/RPKI host size.

## Checks and package evidence

- Native Linux: format, strict all-target locked Clippy; 149 unit passes, one
  existing ignored performance helper, 19 CLI/HTTP/TCP integration passes and
  two load-client passes.
- ARM64 release profile under QEMU: the same 149/19/2 passes, with unchanged
  socket deadlines. This is emulation evidence, not a new native ARM CI pass.
- Fourteen gateway and four separate community submission tests pass.
- Chromium checks pass at 320/390/1440 px for both sites, all seven console views,
  real IPv4/IPv6/AS-set/member queries, source browsing, commands, RPKI explanations
  and the batch form. The independent synthetic console also passes without
  community listings or host credits.
- Public RPKI normal entry points pass: anchor/family totals, covering ROAs,
  VALID/INVALID batch results and malformed/empty/101-route failures.
- Pinned official IRRd oracle: 45 TCP + 45 HTTP WHOIS cases and 71 authoritative
  HTTP decisions, plus 25 policy/router schema cases, 16 delegation schema cases,
  organisation/policy GraphQL checks and filtered NRTMv3 framing cases. The first
  reused oracle retained protected-name tombstones despite source reload; rerunning
  with a fresh isolated PostgreSQL volume passes. Operational databases were not
  changed. The dedicated oracle, preview and console processes are stopped.
- New RPKI fuzz target, with parser dependencies pinned to the root lock versions:
  **347,703 runs in 31 s**, no crash. Corpus/artifacts stay outside Git.
- Locked RustSec scan: no known vulnerable crate; the existing unmaintained
  `rustls-pemfile` warning remains disclosed.

Debian/RPM beta archives build for x86-64 and ARM64. Semver beta ordering maps to
Debian `0.1.0~beta.1-1` and RPM `0.1.0-0.1.beta.1`. Both include deduplicated locked
Linux dependency license texts, with an explicit SPDX reference clarification for
Inflector's missing separate license file. Generated notices are not Git artifacts.

The local binaries were built on Debian 13 and require **glibc 2.39**. Debian
metadata now derives that requirement from ELF symbols instead of incorrectly
claiming 2.36. The x86-64 package installs, imports, serves WHOIS, shuts down and
verifies in a disposable Debian 13 container, including its notice files. Debian
slim excludes documentation by default; the notice-file test explicitly includes
this package's documentation. A Debian 12 ARM container correctly rejects the
newer-glibc artifact. ARM64 QEMU tests on the current sysroot pass; a clean native
ARM package-install pass is not claimed locally. The release workflow builds on
its Debian 12 baseline and gates publication on native x86-64/ARM package jobs.

Local logs are `/tmp/rirrd-beta-*`; screenshots, package artifacts, private test
queues and corpora are under `/var/lib/rirrd-dev/`. None are included in Git.

## DNS, TLS and reachability

The owner added the missing irrd.tools records during this checkpoint. Both apex
names now resolve to `192.67.35.6` and
`2a0f:6280:1d:1940:e6d8:d666:4268:d4c8`. Host IPv4/IPv6 HTTPS returns 200 with
verified hostnames/chains; HTTP redirects 308 to HTTPS. Caddy manages automatic
renewal. Certificates expire 1 February 2027 (rirrd.co.uk 09:30:27 UTC;
irrd.tools 17:58:00 UTC). No www record or additional public port was added.
SSH and the existing public read-only WHOIS port are preserved; write/admin and
RTR listeners remain private. No DNS action remains.

External Globalping HTTPS checks return matching page content over IPv4 and IPv6
for each site. Some individual probes fail, so universal reachability is not
claimed. Measurement IDs:

- rirrd.co.uk IPv4: `2rm5uk0tB1a7JRhBs00021RC4` (Buffalo/São Paulo pass).
- rirrd.co.uk IPv6: `283mBaLjB01oYkL2Q00021RC4` (Los Angeles pass; Lagos fails).
- irrd.tools IPv4: `2q2AWQfBMTwqvPqAD00021RC4` (São Paulo pass; Lagos fails).
- irrd.tools IPv6: `2gSyxtiImCeebtiWP00021RC4` (São Paulo pass; Falkenstein fails).

## Publication boundary

A versioned beta workflow and issue templates are prepared. The workflow verifies
format/Clippy/tests/audit/source hygiene, builds and installs native packages, and
creates a GitHub **prerelease** only after those jobs pass. A source tag does not
prove that a release or its binary artifacts have been published.

The SSH deploy key can push source/tags and remove obsolete branches, but cannot
change repository visibility/settings or read private Actions. `gh auth status`
confirms no account authentication. Public visibility, enabling/checking Issues,
reviewed default-branch integration and verifying the actual GitHub release/CI
require authenticated repository/admin access (`gh auth login` on the VM, or the
GitHub connector). Main has not been directly pushed, force-pushed or merged
without review. Website source availability remains labeled pending until the
repository is actually public. See `tools/release/README.md` for the concrete
publication steps. Do not send tokens in chat.

This beta does not close the documented authentication/account/OpenPGP/mail,
remaining class/reference, NRTMv4 scheduling/publication/recovery, bidirectional
replication, live filtering, broader query or production release-qualification gaps.
