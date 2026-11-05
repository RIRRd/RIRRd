# Testing RIRRd

Run the same daemon and self-hosted console checks locally and in CI:

```sh
python3 tools/testing/run.py --profile core --out /tmp/rirrd-core
python3 tools/testing/run.py --profile web --out /tmp/rirrd-console
```

Keep generated databases, logs, browser profiles, corpora and reports outside Git.
Core runs formatting, strict Clippy, locked Rust unit/integration tests, independent
TCP/HTTP/GraphQL and recovery contracts, and reporting/installer integrity checks.
The browser profile uses the normal Rust binary and a private synthetic two-source
registry, checks queries, sources, schema introspection, accessibility and six
responsive console views. Its network is restricted to loopback. No public
registry updates, live routers or storage servers are involved.

Reports include JSON, JUnit, gate logs and summaries. Skips are not passes; a
fixture failure or a successful runner without case records fails the gate.
GitHub Actions retains results from failed and successful jobs for 14 days.
Linux x86-64, native ARM64 and Windows run core checks. Browser and coverage
jobs run separately. The pinned upstream oracle and real BIRD/FRR parsers have
dedicated workflows. RPSL/NRTMv3 fuzzing and deployment smoke checks run on pushes.
Development branches do not publish packages or releases.

The full-class smoke test imports synthetic objects from all 19 standard IRRd
classes, performs typed TCP lookups and HTTP key browsing after daemon restarts,
and reads every class through bounded exact-key GraphQL batches. It also checks
public export redaction and native database integrity. These minimal mirror
fixtures exercise storage/query coverage, not authoritative validation parity.

## Beta.4 regression coverage

- Each TCP WHOIS request and complete GraphQL operation shares a coherent database
  view while NRTMv3 commits run. Tests pause writers, commit between aliases, cancel
  requests, and verify both the old and subsequently committed answers.
- The large-query oracle fixture expands 1,300 child sets and 5,200 origins. It
  checks complete answers, source precedence, framing, GraphQL selections and
  aliases, explicit resource failures and recovery. Browser tests preview bounded
  portions while retaining the complete downloadable response.
- A shared synthetic allocation/IRT schema corpus is read by Rust and the pinned
  upstream parser. Normal CLI/HTTP tests exercise create/update/delete, both
  maintainer proofs, contact references, route-parent authorization, source
  isolation, whole-batch rollback, journal state and reopen/verify.
- Named source groups and eleven indexed GraphQL filters have entry-point and
  official-oracle comparisons. Sparse intersections inside broad indexes must
  succeed within the finite query-work limit; cancellation remains bounded.

The upstream workflow runs these comparisons on each development push and
relevant PR, and is required by the tagged beta publication workflow. Deliberate
differences from pinned upstream behavior are recorded with the fixture evidence;
an expected difference never turns an arbitrary error into a passing comparison.
See [source queries](compatibility/QUERY_SOURCE_PARITY.md),
[allocations/IRT](compatibility/AUTHORITATIVE_ALLOCATIONS.md) and
[source aliases](operations/SOURCE_ALIASES.md).

## Source coverage

```sh
cargo install --locked --version 0.9.1 cargo-llvm-cov
rustup component add llvm-tools-preview
bash tools/testing/coverage.sh /tmp/rirrd-coverage
```

The enforced source-line floor is 92%. Test modules within src are included;
dependency, fixture and harness code is excluded. Coverage is execution evidence,
not proof of authorization, interoperability or production readiness.

Website/community-platform and operator tests live in the private website repo.
The public daemon and bundled console do not require that repo to build or test.
