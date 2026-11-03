# Source publication preparation — 3 November 2026

The repository remains private. This work prepares a reviewable `codex/` branch;
it does not change visibility, push to main, rewrite history or merge main without
review. Review and merge the intended branch before publishing the default branch.

## Changes completed

- The owner's permissive reuse preference is implemented with the MIT license.
  Cargo, native package metadata and installed notices agree. Dependencies,
  upstream material and separately obtained routing data retain their own terms.
- README, CONTRIBUTING, SECURITY and the AI disclosure explain AI-assisted
  authorship. AI/AI-assisted contributions are welcome, with review and testing
  required. No correctness/security guarantee is inferred from authorship.
- Runtime data, `.env` files, SSH identities, private keys, database files, fuzz
  corpora/artifacts, build caches and logs are excluded from Git. Host SSH keys,
  Caddy certificate state and real downloaded registry snapshots remain outside
  the source trees.
- `tools/check_public_tree.py` audits candidate filenames/sizes/symlinks and
  reachable historical blob paths. It can stage an exact candidate-tree scan.
- Gitleaks 8.30.1 uses the pinned official image digest in the publication workflow.
  Scans run offline with repository mounts read-only, redacted diagnostics and
  default detection rules. Narrow reviewed false-positive exceptions cover only
  a public binary-checksum field and a public PGP identifier in a negative test.
  No complete fixture, directory or benchmark file is excluded from detection.

The local all-reachable-history scan and exact core/site candidate-tree scans
reported zero findings after those reviewed exceptions. No real leaked credential
was identified, so no history rewrite or forced push was performed. The final
integration tree is scanned again after branch integration. Scan reports stay
outside Git; a clean scan is evidence for the checks used, not proof that every
possible secret or vulnerability is absent.

## Verification and publication limits

Native Linux passes formatting, strict Clippy, 138 unit tests, 16 integration
tests and two load-client tests. One ignored unit helper is invoked by the
process-kill integration test. The same release tests pass under ARM64 QEMU.
The locked dependency scan checks 227 crates against 1,403 RustSec advisories.
The expanded snapshot-parser fuzz smoke completed 221,757 runs in 31 seconds.
The official pinned oracle still matches 39 TCP and 39 HTTP WHOIS cases, 47
authoritative HTTP decisions and the documented GraphQL class/field fixtures.
The container runtime passes unprivileged/read-only/network-isolated UI, query,
license and shutdown/reopen checks. These are local results, not new CI results.

Authenticated GitHub Actions/repository-settings access is unavailable to this
session's deploy key. New private Actions runs, branch-protection settings and
private vulnerability-reporting settings cannot be independently verified here.
Historical CI links remain historical; no new passing CI link is fabricated.

The public test registry is experimental. Full authentication/account/class
parity, continuous mirroring/publication/recovery, live RPKI/scope/preference
semantics, broader queries and release/security/load qualification remain open.
Source-publication hygiene and production-readiness are separate decisions.
