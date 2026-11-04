# Testing RIRRd

Run the same gates locally and in GitHub Actions. These checks exercise the
implemented beta; they do not establish full IRRd parity. The
[compatibility inventory](compatibility/IRRD_FEATURE_MATRIX.md) remains the
list of open behavior and release requirements.

## Standard build checks

```sh
python3 tools/testing/run.py --profile core --out /tmp/rirrd-core
python3 tools/testing/run.py --profile web --out /tmp/rirrd-web
```

Use a new or existing output directory **outside the repository**. Each run
produces `report.json`, `junit.xml`, `summary.md`, individual case JSON/JUnit
files and gate logs. A failed command, failed case, absent case results or
blocked protocol gate causes a failing exit status. Gates continue where safe
so one failure does not hide unrelated results. The GitHub job summary shows
the same results; artifacts are retained for 14 days, including failed runs.
Do not put generated databases, queues, logs or corpora in Git.

Core requires Git, Python 3.11+ and stable Rust with rustfmt/Clippy. The Rust
TLS dependencies need the usual native C compiler/linker. Linux additionally
runs the public gateway and private community service tests. Windows executes
Rust, native protocol/recovery contracts and the report-integrity suite; a
POSIX executable-fixture reporting test is explicitly skipped there.

Web also requires Chromium or Chrome and Python Markdown. On Debian/Ubuntu,
install the supported `python3-markdown` package and run the driver with
`/usr/bin/python3` if a separate Python installation cannot see that package.
The suite uses the repository's static builders and the normal binary. There
is no JavaScript package install or external font/script dependency.

## Coverage map

| Area | Main evidence | Normal entry point exercised |
| --- | --- | --- |
| Authoritative authentication and RPSL references | `tests/smoke.rs`, submission/storage/RPSL unit suites | CLI and loopback HTTP; mixed-batch rollback, source denial, maintainer rotation |
| Exact, covering, recursive and inverse queries | Rust WHOIS/GraphQL tests plus `tools/testing/protocols.py` | TCP, HTTP WHOIS, GraphQL, both address families |
| Wire behavior and isolation | Protocol contracts | Byte lengths including UTF-8, persistent pipelines, isolated source sessions, concurrent readers, malformed and oversized inputs |
| Credential redaction and API errors | Core and protocol suites, gateway suite | WHOIS/GraphQL, invalid Host, disabled submission, unavailable RPKI, key pagination |
| Restart and recovery | Core and protocol suites | Real process restart, failed import preserving data, verified backup restore, corruption/overwrite rejection |
| NRTMv3 and NRTMv4 | Rust unit/integration suites; pinned oracle workflow | CLI/TCP, scheduled retries, filtered replay, source serials, signed snapshot/delta verification |
| RPKI and RTR | Rust RPKI/service suites; protocol unavailable-state contract | Independent full-scan verdicts, AS0, max lengths, bounded explanations, batch failures, RTR reset/delta/unavailable wire states |
| Websites and standalone console | `tools/testing/web.py`; existing console/router workflow | Built project/community sites, real browser queries, mobile pages, metadata, schema workbench, private-only node submission |
| Gateway and node review queue | `tools/live-service/test_gateway.py`, `site/community/test_service.py` | Read-only allowlists, framing, rate limits, history persistence, validation, private queue and retention |
| Reporting integrity | `tools/testing/test_*.py` | Setup failure, subtest failure, expected failures/skips, deliberate build failure, dirty/inflated result rejection |
| BIRD/FRR, pinned upstream and packages | Dedicated workflows | Real bgpq4 and router parsers, official IRRd oracle, DEB/RPM install/bootstrap and systemd |

The protocol corpus adds 56 separately reported contracts against a fresh
synthetic two-source registry. Browser cases use 14 actual fixture objects,
not the production database. Both address families and two sources allow
explicit checks for incorrect covering results and source leakage.

## Isolation and cleanup

Fixtures use RFC documentation prefixes/ASNs and the existing synthetic router
fixture. Registry listeners bind **127.0.0.1 on ephemeral ports**. The fixture
process reports its allocated addresses before a client connects, avoiding
fixed-port races. Test databases and a fresh submission queue are private and
removed in cleanup. Browser traffic uses a closed external proxy with loopback
bypass; checks reject non-loopback HTTP requests. Websites query the real
fixture daemon through the actual read-only gateway. No public registry update,
production queue submission, live RPKI collection or production source import
is needed for these gates.

Only the RPKI-disabled failure path runs in this external fixture. Validated,
expired, AS0, covering and RTR behavior is exercised with private synthetic
payloads inside the Rust service tests. Real global validation and bidirectional
upstream mirroring need separate interoperability/release evidence; a disabled
validator test is not evidence of complete validation coverage.

## CI cadence

`.github/workflows/ci.yml` runs on every push and pull request, plus manual
runs. It covers Linux x86-64, native Linux ARM64 and Windows core checks;
independent browser and coverage jobs; and the locked dependency audit.
`deployment-smoke.yml` also runs on every push/PR. `fuzz.yml` runs 15 seconds
per RPSL, NRTMv3 and RPKI target per push/PR, with 120 seconds each in the weekly
run. Reproducing crash inputs are retained on failure, outside Git.

The existing pinned upstream, real router tools, native DEB/RPM and source
publication workflows retain their dedicated triggers. Beta publication remains
an explicit tag workflow. Tests do not push to main, merge a PR, change the
repository's visibility or deploy a public registry.

All third-party Actions are pinned to reviewed commit hashes. Runtime/build
packages follow their existing pinned or distribution-supported profiles.
Concurrent obsolete runs can be cancelled; cancellation is not a passing build.

## Source coverage

Install the pinned development tool and LLVM tools, then run:

```sh
cargo install --locked --version 0.9.1 cargo-llvm-cov
rustup component add llvm-tools-preview
bash tools/testing/coverage.sh /tmp/rirrd-coverage
```

The script follows the tool's [external-test recipe](https://github.com/taiki-e/cargo-llvm-cov/tree/v0.9.1#external-tests).
It instruments Rust builds, runs the locked Rust suite and the independent
protocol corpus, then exports JSON, LCOV and an HTML source drilldown. Only
project `src/*.rs` files enter the summary; dependency, external fixture and
harness files are excluded. Test modules within `src` are included in the line
coverage definition. The first measured baseline was 93.13%; CI enforces a 92% source-line floor.
Line coverage records execution; it does not prove correct
authorization, interoperability, timing or every input combination.

## Public verification snapshot

The project page at <https://rirrd.co.uk/testing/> shows a recorded snapshot.
The snapshot identifies the exact tested revision and whether results came from
local execution or GitHub Actions. It does not label an unauthenticated private
Actions run green. CI-generated JSON/JUnit and HTML coverage artifacts are
available from the workflow run to authorized repository users.

To publish a new snapshot after testing a clean committed revision:

```sh
python3 tools/testing/publish.py --core /tmp/rirrd-core/report.json \
  --web /tmp/rirrd-web/report.json --coverage /tmp/rirrd-coverage/summary.json \
  --output site/project/verification.json
```

Reports must refer to the same clean, reachable source commit. Counts and gate
exit codes must agree. Skips are never counted as passes. JSON omits raw failure
traces, visitor addresses, operator contacts, credentials and runtime paths.
Full diagnostic traces remain in private local/CI artifacts. Commit the snapshot
and use the existing static deployment script. Formatting-only or evidence
commits after the tested revision are distinguished by the site's build manifest.
