# Public source and release hygiene

The MIT daemon and bundled self-hosted console are public at
[RIRRd/RIRRd](https://github.com/RIRRd/RIRRd). Project/community websites and
sponsored-node operations are maintained in a separate private repository.
This page replaces the pre-publication checklist; historical qualification
belongs to the exact commits recorded in the release notes.

## Publication checks

- Keep runtime databases, downloaded registry fixtures, credentials, SSH private
  keys, certificate state, caches and logs outside Git and release archives.
- Run `python3 tools/check_public_tree.py` and the pinned Gitleaks full-history
  scan from the release workflow. Reviewed synthetic public-certificate fixtures
  have exact false-positive exceptions; directories and fixture collections are
  not blanket-excluded. Secret-canary tests check that detection remains active.
- Preserve MIT licensing, third-party notices, AI-assisted development disclosure,
  issue templates and private vulnerability reporting. A clean scan is evidence
  for its rules, not a guarantee that every possible secret has been detected.
- Build from the reviewed commit on protected main. The beta tag must match the
  Cargo version. Required core, browser, audit, coverage, oracle, router, container,
  fuzz and native-package checks gate publication. Failed checks block release.
- Publish an explicit GitHub prerelease, verify actual server asset names and
  SHA-256 digests, then update website download metadata. Keep measured test
  reports tied to their tested source commit, never to an untested version label.

Use organization-scoped GitHub credentials kept outside the repository. Do not
force-push main, rewrite historical tags or bypass required checks. Source review,
public visibility and full production qualification are separate matters.

See [release procedure](../tools/release/README.md), [beta status](BETA_STATUS.md)
and [testing](TESTING.md). RPKI validation and RTR are outside RIRRd's IRR-only
scope. Remaining authentication, query, policy and mirroring gaps are documented
in the compatibility matrix.
