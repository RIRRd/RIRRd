# Security

RIRRd is development software. Full IRRd parity and production qualification
remain incomplete. The public project website is separate from registry services.
An explicitly authorized experimental read-only service is public on irrd.tools; rirrd.co.uk is the separate project showcase.
Its submissions, journal/mirroring publication and administration remain disabled.
Development interfaces stay on loopback or isolated container networks. Do not use benchmark results as a production-readiness claim.

## Reporting

Do not put live credentials, private keys, production registry data or a working
exploit against another operator into a public issue. Use
[GitHub private vulnerability reporting](https://github.com/RIRRd/RIRRd/security/advisories/new),
which is enabled for the public project repository. No public security response SLA or unverified
security email address is announced at this checkpoint.

## Source publication hygiene

Before publication, scan the current tree and all reachable Git history with a
redacting secret scanner, review findings, check tracked file types/sizes and
verify that runtime directories are excluded. A clean scanner result is one
piece of evidence, not a guarantee that no secret or vulnerability exists.

The scanner configuration retains default rules. Its narrow exceptions cover
public binary SHA-256 metadata and a public PGP key identifier used to test an
unsupported authentication scheme. They do not allow private keys, deployment
credentials, arbitrary test directories or complete benchmark files.

If a real credential enters Git, revoke/rotate it and assess reachable history
and hosted copies before publication. Removing it from the latest file alone is
insufficient. Do not rewrite protected/shared history or force-push main as an
unreviewed cleanup step.

AI assistance does not waive source review, testing, dependency audits, access
control or secret handling. See [the AI disclosure](docs/AI_DISCLOSURE.md).
