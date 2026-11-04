# RIRRd 0.1.0-beta.2

RIRRd is beta software. It aims to reduce the resources needed for a self-hosted
Internet Routing Registry. This beta
is useful for isolated evaluation and read-only public tests; it is not a claim
of full IRRd parity, security qualification or production readiness.

## Available for evaluation

- Embedded transactional IRR storage and bounded TCP WHOIS, HTTP WHOIS and GraphQL.
- Source selection, IPv4/IPv6 origin queries, AS-set traversal and policy object browsing.
- NRTMv3 mirroring with atomic updates and source serial tracking.
- Snapshot import/export, verified backup/restore and bounded authoritative updates.
- Independent self-hosted console and separate community website at irrd.tools.
- Source, activity, resource graphs from observed data.

The public UK node currently mirrors nineteen routing sources. Seven follow
NRTMv3 every sixty seconds; twelve use daily snapshots. Contacts, accounts and
other non-routing classes are outside this production mirror profile.
The development full-data profile retains all 19 standard IRRd classes and has
its own storage and source cursors. See live telemetry for the serving profile;
an offline import is not evidence that a generation is already serving.
A source being loaded does not establish that its last check succeeded.

## Known gaps

Authentication/account workflows, remaining class/reference behavior, OpenPGP,
mail submission and notification workflows need further parity work. NRTMv4
publication, scheduling, rollover, retention, recovery and bidirectional upstream
interoperability are incomplete. Broader WHOIS/GraphQL behavior, live scope/route-preference suppression and journal transitions remain open.

The recorded 100,000-object benchmark uses historical tested commits, IRR-only
profiles and synthetic data. It does not establish current release capacity. Linux x86-64 and ARM64 checks must be assessed
at the commits and environments documented in the source repository.

## Deliberate IRR-only scope

RIRRd does not bundle an RPKI validator, ROA-file checker, origin-validation API
or RTR service. These experimental components were removed in beta.2 to focus
on lightweight IRR hosting. Operators can use a separate established validator.
Old `[rpki]`, `authoritative_roa_file` and `authoritative_roa_exempt_sources`
settings are rejected; remove them before upgrading. Stored IRR objects and
mirror serials are preserved. The compatibility field `rpkiRovFilter` is always
false. Mirrored `rpki-ov-state` attributes are stripped from public object output
and rejected in authoritative updates rather than trusted as validation results.

## Report a problem

Use [GitHub Issues](https://github.com/RIRRd/RIRRd/issues) for reproducible reports. Include the version, OS/architecture, a minimal synthetic configuration,
request/response, expected behavior, reproduction steps and relevant redacted
logs. Performance reports should state the dataset, concurrency and measurements.
Do not include private keys, credentials, real private contacts or runtime databases.

For a suspected vulnerability, follow SECURITY.md and GitHub private vulnerability
reporting when enabled. Do not publish a working secret or private security report
in a public issue. There is no invented security contact address.

## License and contributions

Project code is MIT licensed. Dependencies and imported registry data retain their
own licenses and provenance. RIRRd is built with AI assistance. Human and AI-assisted
contributions are welcome; every change needs review, reproducible tests and clear
provenance. Authorship is not a correctness or security guarantee.
