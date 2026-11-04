# IRR-only beta.2 checkpoint — 4 November 2026

The project owner narrowed RIRRd to lightweight IRR hosting. Embedded RPKI
validation, RTR, origin-check APIs and ROA-file admission are removed. Use a
separate validator for cryptographic origin validation. This is a deliberate
scope difference from the pinned IRRd 4.5.4 oracle, not completed RPKI parity.

## Exact tested source

`9dc614a58a8aafb54cc1d8b9252e58d42173d4a3`, Linux x86-64, stable Rust.
Installed binary SHA-256:
`d554129672e16938f7bed9518db3349794e2490ed64cd07a107de5c36e1d6da2`.

- Rust: 161 passed, one long-running soak test intentionally ignored.
- Independent protocol/recovery: 56 passed.
- Reporting and installer: 23 passed.
- Gateway: 17 passed, including removed endpoints and history migration.
- Private community queue: four passed.
- Real-browser project/community sites: 14 passed, including retired-tab fallback.
- Source-line coverage: 94.44% (12,077 / 12,788); enforced floor 92%.
- Locked dependency audit: no vulnerable dependencies; RustSec `965bf9ad083b58e6340d65829d85217b18be9290`.
- Dependency graph: 340 to 229 non-root packages across all locked targets;
  111 removed, no retained package versions changed.
- Local x86-64 DEB/RPM build passed, including generated dependency licenses.

These are local measurements. The public verification snapshot distinguishes
local results from independently linked GitHub Actions evidence.

## Pinned upstream comparisons

Official IRRd v4.5.4 at
`8e8080dab744a348b7d0e6efcb53f403301dd075`, isolated PostgreSQL/Redis and a fresh
synthetic fixture. WHOIS and HTTP ports bound only to loopback; no public updates.

45/45 TCP and 45/45 HTTP WHOIS cases matched. 71 HTTP authorization cases
matched. Organisation schema (10), policy/router schema (25), AS-block/domain
schema (16), filtered NRTMv3 parsing (five), mirrored control-character admission,
organisation GraphQL values/nine types and policy/router values/32 types passed.
The dedicated oracle containers and their disposable PostgreSQL volume were
removed after testing.

## Live node migration

The 19-source generation and database were retained. No reimport, format change
or registry data purge was needed. Configuration and runtime backups were taken
before replacement. Native HTTP inventory and representative WHOIS queries
recovered in 3.332 seconds; Caddy and community services were preserved.

The executable changed from 22,852,136 to
15,306,512 bytes (33.0% smaller). Retired native and public
validator endpoints return 404, and TCP 3323 is not listening. Refresh tools and
the source catalog no longer generate a validator configuration.
The retired downloadable validator cache (2,607,457,984 bytes)
was removed; IRR snapshots and database data were kept.

At 2026-11-04T10:23:16Z, after restart and representative IPv4/IPv6
queries, the live process reported 16080 kB RSS and
13773 kB PSS. This is a point-in-time live
observation, not a controlled comparison or a minimum-RAM guarantee. Mirroring,
imports, query working sets and OS cache have separate resource costs.

## Reproduction

```sh
CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 python3 tools/testing/run.py --profile core --out /tmp/rirrd-core
CARGO_BUILD_JOBS=4 /usr/bin/python3 tools/testing/run.py --profile web --out /tmp/rirrd-web
CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 bash tools/testing/coverage.sh /tmp/rirrd-coverage
cargo audit --db /path/to/rustsec-advisory-db --no-fetch
bash deploy/packages/build.sh target/release/rust-irrd /tmp/rirrd-packages
```

See `tools/oracle/README.md` and `.github/workflows/upstream-oracle.yml` for the
fresh-volume bootstrap, schema checks and normal-entry differential commands.
All generated fixtures, logs, dependency caches and databases stay outside Git.

## Remaining scope

Full IRRd parity is incomplete: account/OpenPGP/API-token/mail workflows,
remaining class/reference decisions, NRTMv4 publication/scheduling/recovery,
broader queries, live scope/route-preference semantics and sustained release
qualification. Native ARM64 and Windows evidence must be assessed from their
actual CI runs. No historical performance number is relabeled as beta.2 capacity.

## Public release qualification

[Beta.2 release workflow](https://github.com/RIRRd/RIRRd/actions/runs/41813943576)
passed all 11 jobs at the tested source revision: Linux x86-64, native ARM64 and
Windows core, browser cases, source coverage, dependency audit, workflow syntax,
history scan, both native package profiles and publication. Native DEB/RPM
installation/bootstrap and real systemd startup were exercised by the package jobs.
The [public prerelease](https://github.com/RIRRd/RIRRd/releases/tag/v0.1.0-beta.2)
remains explicitly beta. Post-publication filename normalization was corrected
in the installer/checksum manifest; future publication checks compare the actual
server asset names and digests. Use the installer attached to the release.

## Public network verification limit

From the VM, both domains resolve to the intended IPv4/IPv6 addresses. Both
HTTPS pages return 200 with valid Let's Encrypt certificates; HTTP redirects
308 to HTTPS; retired validator paths return 404. Public-address WHOIS works
over both families. The 19-source generation serves 4,994,092 routing objects
at this observation. Browser/mobile/chart checks pass on both deployed sites.

A GitHub-hosted external runner timed out establishing IPv4 HTTPS to the project
domain in two attempts. This is an unresolved external reachability check, not
an external pass. The VM listens on ports 80/443, its input rules allow those
ports and SSH access is preserved. Azure artifact-storage endpoints also time
out from the VM; artifacts could not be downloaded here. Provider routing/ACL
checks and a fresh external test are still needed. No host firewall broadening
was performed to conceal that result.
