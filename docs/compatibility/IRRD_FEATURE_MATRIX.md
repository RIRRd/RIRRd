# IRRd 4 compatibility inventory

The reference is **IRRd v4.5.4**, released 19 October 2026, at commit
`8e8080dab744a348b7d0e6efcb53f403301dd075`. On 3 November 2026, GitHub's
latest-release API, PyPI and the stable manual agreed on the version, and
`git ls-remote --tags` and the checked-out tag agreed on the commit. Pin the
oracle to this SHA and record the oracle configuration for every test run.
[Release](https://github.com/irrdnet/irrd/releases/tag/v4.5.4) ·
[Commit](https://github.com/irrdnet/irrd/commit/8e8080dab744a348b7d0e6efcb53f403301dd075) ·
[PyPI](https://pypi.org/project/irrd/4.5.4/) ·
[Manual](https://irrd.readthedocs.io/en/stable/)

The reference uses Python worker processes, PostgreSQL for durable state,
and Redis for query preloads and event signalling. The compatibility
contract covers observable interfaces and operator workflows. The Rust
implementation can use a different storage format and process model.
Upstream is BSD-2-Clause and its license file lists additional vendor
notices. The inventory uses upstream as a behaviour reference. Review the
license and preserve relevant notices before copying any code or fixtures.
[Architecture][arch] · [Storage][storage] · [License][lic]

**Status.** Open means compatibility evidence is pending. Partial means a
bounded subset works and has local evidence. Verified requires local tests,
oracle results or protocol interop evidence as appropriate. Release
extensions are tracked separately from upstream behaviour. Each row stays
open until the stated behaviour and safety checks are demonstrated.

## RPSL and storage

| Capability | Reference and specification | Rust implementation and compatibility target | Security requirement | Required evidence | Status |
| --- | --- | --- | --- | --- | --- |
| Parsing and rendering | [Parser][parser], [object definitions][objects], [RFC 2622][2622], [RFC 4012][4012] | Parse object boundaries, continuation by whitespace or plus, comments and blank lines, repeated attributes and encoding. Preserve source text and separately compute canonical fields. Match strict submission and tolerant mirror behaviour, including per-object diagnostics. | Bound object, line and attribute counts, avoid crashes and redact credentials from diagnostics. | Byte and parsed-field oracle cases for CRLF, folds, comments, unknown fields, encodings, duplicates, truncation and invalid records, plus property and fuzz runs. | Partial. Bounded UTF-8 parsing, identity normalization and public auth redaction have [local unit tests](../../src/rpsl.rs). Strict/tolerant field semantics and differential tests are pending. |
| Object classes and schema | [Pinned class mapping][objects], [RFC 2622][2622], [RFC 4012][4012], [RFC 2725][2725] | Cover all 19 classes, listed below. Define per-class attributes, keys, validation, multiplicity, normalization, searchability and strong versus weak references. | Enforce required and protected fields in authoritative changes. | Class-by-class oracle cases for valid records, missing fields, duplicates, wrong references and canonical serialization. | Partial. [Rust parser](../../src/rpsl.rs) recognizes all 19 classes and normalizes identity fields. Full attribute, multiplicity, reference and policy validation remain open. |
| Durable objects and indexes | [Storage design][storage], [models][models] | Atomic object, index and metadata changes, keyed by source, class and RPSL key. Index IP ranges, prefixes, ASN, origin, maintainers, references, inverse fields and set membership. Snapshot reads during write transactions. | Preserve committed data and roll back interrupted writes. Version and validate database format. | Reopen and kill tests, transaction race tests, indexed versus brute-force query comparison and large mixed workloads. | Partial. [Storage tests](../../src/storage.rs) cover committed reopen, integrity, failed replacement and reader snapshots. Current indexes support exact key and route-origin lookup. The primary-key index also supports exact IPv4 and IPv6 prefix ranges. Crash and scale qualification remain open. |
| Journal and source status | [Storage][storage], [models][models], [mirroring][mirroring] | Track source-level mirror and export serials, journal retention, global event order and NRTM state. Preserve origin and visibility metadata and publish only committed changes. | Prevent cross-source and pre-commit leakage, reject stale or inconsistent state. | Serial gap, expiry, rollback, restart and suppression-transition fixtures. | Partial. Local NRTMv3 serials persist transactionally with all index changes. Bounded NRTMv3 journal history and logical verification are implemented. Export serials and event state remain open. |

The pinned mapping covers these 19 classes.

| Family | Classes | Key and validation focus |
| --- | --- | --- |
| Autonomous systems | `as-block`, `aut-num`, `as-set` | ASN boundaries, ranges, hierarchical set names, policy attributes and implicit membership. |
| Routing and routing policy | `route`, `route6`, `route-set`, `rtr-set`, `filter-set`, `peering-set`, `inet-rtr` | Route keys include prefix and origin, prefix family and canonicalization, multiprotocol membership, policy and references. |
| Address and DNS registrations | `inetnum`, `inet6num`, `domain` | IPv4 ranges, IPv6 network, canonical labels and inverse lookups. |
| Ownership and contacts | `person`, `role`, `organisation`, `irt`, `mntner`, `key-cert` | Contact handles and typed references, maintainer auth and lifecycle, key fingerprints and certification data. |

The per-attribute authority is the field declaration and validators in the
pinned object definitions. The [machine-readable inventory](IRRD_4_5_4_RPSL_ATTRIBUTES.json) records 259
declarations, including types, multiplicity, keys and references. Compare
it against Rust declarations and separately exercise runtime validators
before marking the object schema verified.
[Object definitions][objects]

## Query interfaces

| Capability | Reference and specification | Rust implementation and compatibility target | Security requirement | Required evidence | Status |
| --- | --- | --- | --- | --- | --- |
| WHOIS transports and framing | [WHOIS manual][whois], [RFC 3912][3912] | TCP and HTTP `/v1/whois/`. Reproduce separate RIPE-style and IRRd-style response framing, empty and error cases, CRLF, byte lengths and TCP connection mode. | Bounded read, timeout, connections and responses. Apply access policies consistently. | Byte-level and connection transcript comparisons, including non-ASCII, truncation, pipelining and slow clients. | Partial. [TCP tests](../../tests/smoke.rs) cover one-shot exact, no entries, sanitized object and unsupported query. Persistent TCP sessions and per-source query state now have local coverage. WHOIS over HTTP and oracle framing remain open. |
| IRRd command language | [WHOIS manual][whois] | Support `!!`, `!t`, `!a`, `!g`, `!6`, `!i`, `!j`, `!J`, `!m`, `!o`, `!n`, `!r`, `!s`, `!e`, `!v`, and the `!fno-*-filter` commands. Track source selection and recursion exclusions per connection. | Bound recursion, visited nodes and output. Redact authentication fields. | Oracle transcript for each command, each `!r` option, source priority, source aliases, state changes and filter controls. | Partial. Local tests cover exact keys, origins, covering and covered prefix lookups, inverse attributes, persistent source selection and bounded set expansion. The upstream runtime oracle and remaining commands are open. |
| RIPE-style language | [WHOIS manual][whois] | Support `-l`, `-L`, `-M`, `-x`, `-i`, `-t`, `-q sources`, `-g`, `-k`, `-s`, `-a`, `-T`, `-K`, `-V` and free text. Match query precedence and response ordering. | Budget broad lookups, gate NRTM separately and filter sensitive content. | Golden queries for modifier combinations, ranges, class templates, malformed commands and mirror ACLs. | Open |
| Set and prefix resolution | [Query resolver][resolver], [RFC 2622][2622], [RFC 4012][4012] | Shared resolver for exact, covering and covered prefixes, origin queries, explicit and indirect set membership, source precedence and aliases. | Bound recursion and enforce per-query budgets across both APIs. | Directed cycles, duplicate memberships, cross-source aliases, IPv4/v6 boundaries, malformed ranges and ordering comparison. | Partial. Local storage and TCP tests cover bounded origin, exact, covering and covered prefix queries, and explicit and indirect set expansion. Full interoperability remains open. |
| GraphQL schema and results | [GraphQL manual][graphql], [pinned schema generator][schema], [GraphQL specification][gqlspec] | Match `rpslObjects`, `databaseStatus`, `asnPrefixes`, `asSetPrefixes` and `recursiveSetMembers`, including ASN/IP scalars, generated RPSL types, interfaces, contacts union, filters, `recordLimit`, relationship traversal, source/status fields and restricted journal resolution. | Cost limits include aliases and fragments. Enforce bounded depth, request size, output, time and concurrency. Restrict traces and journal data. | Canonicalized introspection diff and response/error cases, budget and ACL tests, batched resolver load tests. | Partial. [GraphQL tests](../../src/graphql.rs) cover bounded exact keys, configured-source/class filters, common and typed fields, redaction and alias cost. ASN and AS-set prefix roots have local tests. Remaining roots, filters, relationships and schema parity remain open. |
| GraphQL explorer | [HTTP routes][http], [v4.5.4 release notes][release] | Serve GraphiQL from `/graphql/` with local assets. | Content Security Policy and no external script dependency. | Browser query and CSP validation. | Open |

For `!m` exact lookups, the pinned manual describes the key as case
sensitive. The pinned query resolver delegates to the storage query,
which uppercases and trims keys. Route and route6 queries also remove
spaces and dashes from legacy composite keys. The
[differential harness](../benchmarks/METHODOLOGY.md#running-exact-key-whois-differential-checks)
includes cases for each behavior. [WHOIS manual][whois] ·
[Query resolver][resolver] · [Storage key query][pk-query]

## Mirroring, exports and event delivery

| Capability | Reference and specification | Rust implementation and compatibility target | Security requirement | Required evidence | Status |
| --- | --- | --- | --- | --- | --- |
| Full snapshots | [Mirroring manual][mirroring], [configuration][config] | Local and remote files, gzip, per-source class filters, expected source, optional serial import, full replacement and scheduler retry. Respect source priority and health. | Bound compressed and decompressed input. Protect local paths and remote URL resolution, redirects and timeouts. Publish source atomically. | Compare plain/gzip imports and rejection counts. Inject invalid source, malformed records, short transfer and interruption. | Partial. Local plain/gzip import and rollback are covered by [import](../../src/import.rs) and [smoke](../../tests/smoke.rs) tests. Remote fetch, scheduling and mirror serials remain open. |
| NRTMv3 client and server | [Mirroring manual][mirroring], [RIPE NCC NRTM protocol][ripe-nrtm] | Request and respond to `-g SOURCE:3:start-end`, enforce serial continuity, operation and object handling, and retain local/upstream serial mapping. Support full reload on gaps. | Mirror access controls, safe bounds and no journal secret exposure. | Bidirectional pinned-oracle interop with gap, ACL, rollover and reload cases. | Partial. Local and TCP client tests cover finite unfiltered contiguous transcripts, atomic serial updates, rollback and deadlines. A Rust-to-Rust test exercises retained ranges, source ACLs, redaction and no-update replies. Filtered gaps and upstream interop remain open. |
| NRTMv4 client | [Mirroring manual][mirroring], [draft -07][nrtm07], [RFC 7464][7464], [RFC 7515][7515] | Verify signed update notification, check snapshot and delta digests, process JSON Text Sequences, apply session/version changes, key rotation and persistent rollback detection. | Verify before applying. Restrict remote retrieval and prevent replays, downgrade, key substitution and decompression bombs. | Oracle server interop, tamper, stale/future notification, restart, missing delta, rotation and replay tests. | Open |
| NRTMv4 server | [Mirroring manual][mirroring], [draft -07][nrtm07], [RFC 7464][7464], [RFC 7515][7515] | Publish signed notifications after versioned snapshot/delta files become durable. Match baseline profile and expiry windows. | Protect signing key, prevent path traversal, publish atomically and redact exports. | Oracle client interop and publication failure, rotation and stale-file tests. | Open |
| NRTMv4 IETF evolution | [IETF Datatracker][nrtm-current] | IRRd 4.5.4 documents draft -07. Datatracker returned revision -11 during research. Compare and test wire differences before enabling another profile. | Prevent cross-profile downgrade and ambiguous acceptance. | Track exact document version and capture bidirectional vectors for every supported profile. | Open |
| Bulk exports | [Mirroring manual][mirroring] | Scheduled compressed, per-source RPSL exports and companion serial state with public and restricted variants where applicable. | Keep unfiltered exports private and credentials out of public output. | Import exported files into both implementations and verify object counts, hashes and publication consistency. | Open |
| Event stream | [Event stream manual][event], [RFC 6455][6455] | Initial download at `/v1/event-stream/initial/` and WebSocket updates at `/v1/event-stream/`, using snapshot boundary and resumable global journal position. | ACL, sanitization, bounded buffers and slow-client handling. | Protocol transcript, reconnect, concurrent snapshot boundary, retention gap and ACL cases. | Open |

## Authoritative updates and route policy

| Capability | Reference and specification | Rust implementation and compatibility target | Security requirement | Required evidence | Status |
| --- | --- | --- | --- | --- | --- |
| Submission and reports | [Database changes manual][changes], [HTTP routes][http], [RFC 2725][2725] | Create/update/delete via HTTPS, CLI, web and inbound mail. Match validation, per-object outcomes and notification semantics for multi-object batches. Keep authoritative flags and separate source policy. | Explicit authorization on every object, safe dry-run semantics and no plaintext secrets in feedback. | Differential validation and multi-object cases for success, rejection, references between objects and transaction failure. | Open |
| Maintainer authorization | [Database changes manual][changes], [object validation][validation], [RFC 2725][2725], [RFC 9580][9580] | Match old/new `mnt-by`, parent and route requirements, overrides, PGP signatures and key-certs, legacy password formats and their configured policy, API key scope and account authentication. | Bound total verification work and hash cost, including legacy stored hashes. Prevent cross-source authorization and strip credentials. | Permission matrix, known hashes, invalid signatures and UUIDs, source isolation and bounded-work tests from v4.5.4 security fixes. | Open |
| Protected names, references and suspension | [Object validation][validation], [suspension][suspension] | Validate strong and weak references. Protect deleted person, role and maintainer names. Support protected-name override, maintainer suspension and reactivation with status/journal handling. | Enforce operator-only controls and preserve audit history. | Oracle cases for attempted name takeover, in-use deletes, overrides and suspension/reactivation. | Open |
| Notifications and history | [Database changes manual][changes], [web UI][ui], [RFC 5322][5322] | Match submission result format, contact discovery, notification reason, auth redaction and source-scoped change history. | Validate and unfold email headers safely, scope historical access and bound deliveries. | Recipient, malformed mail, multi-address header, cross-source history and repeat-notification tests. | Open |
| ROA ingestion and ROV | [RPKI integration][rpki], [RFC 6811][6811], [RFC 8481][8481], [RFC 9582][9582] | Periodically ingest validator JSON, atomically recompute valid/invalid/not-found and handle maxLength and ASN 0. Apply source exceptions, pseudo-source `RPKI`, `rpki-ov-state`, update checks and optional notifications. | Validate input and preserve last coherent dataset on fetch or parse failure. Reject a conflicting real `RPKI` source when enabled. | Compare overlaps, IP families, no covering ROA, AS mismatch, ASN 0, maxLength, exclusions, pseudo-objects and outage/recovery. | Partial. The local ROV CLI evaluates a bounded validator JSON feed for IPv4 and IPv6, including overlaps, maximum length and AS0. Periodic ingestion, live filtering, pseudo-objects and outage recovery remain open. |
| Scope filtering | [Scope filtering][scope] | Apply configured ASN and prefix exclusions with per-source opt-outs. Update status, normal query visibility and journal transitions. | Keep override permission explicit and avoid leaking suppressed data into default exports. | Boundary, source exception and filter change comparisons. | Open |
| Route preference and combined suppression | [Route preference][pref], [suppression overview][suppression] | Resolve conflicting route objects by configured source preferences and apply suppression consistently across query, export and journal surfaces. | Ensure consistent data version and prevent visible journal from diverging from visible objects. | Overlap/tie, preference changes, interaction with ROV/scope and source serial policy comparisons. | Open |

## Operator workflows and release gates

| Capability | Reference and specification | Rust implementation and compatibility target | Security requirement | Required evidence | Status |
| --- | --- | --- | --- | --- | --- |
| Configuration and service lifecycle | [Configuration][config], [architecture][arch] | Validate source defaults, aliases, mirror/auth options, listeners, ACLs, schedules and resource limits. Map compatible upstream configuration and report unsupported settings. | Validate before opening listeners or modifying state. Protect secrets and use least privilege. | Validation cases, lifecycle, reload and graceful shutdown tests and config migration report. | Partial. [Smoke test](../../tests/smoke.rs) verifies TOML configuration checking before state creation and starts the TCP service. Reload and upstream configuration migration remain open. |
| HTTP status, metrics and logging | [HTTP routes][http], [metrics manual][metrics] | Compatible status and source fields where used by integrations. Prometheus metrics, structured logs and separate readiness and liveness. | Enforce endpoint policy and avoid secrets or user-controlled labels of unbounded cardinality. | Schema and scrape cases, unhealthy storage and stale source cases, secret exposure checks. | Open |
| Web UI and account management | [Web UI manual][ui], [RFC 6238][6238], [WebAuthn][webauthn] | Provide account creation, login/reset, TOTP and WebAuthn, maintainer migration, permissions, tokens, object views and change log. Extend with operator source and health controls. | Strong sessions, CSRF, MFA replay protection, scoped change logs, restricted administrative routes and accessibility checks. | Browser journeys, replay/session/fixation tests, source isolation and accessibility audit. | Open |
| CLI maintenance | [Pinned CLI entry points][cli], [architecture][arch] | Provide safe lifecycle, config checks, load/update/export/status, force reload, journal maintenance, key generation and database integrity/maintenance commands. | Require explicit opt-in for destructive operations and restrict privileges. | Exit status, machine output, recovery, offline operation and secret redaction tests. | Partial. The [CLI](../../src/main.rs) provides server, config check, database verify and snapshot import. The [smoke test](../../tests/smoke.rs) exercises each. |
| Backup and migration | [Availability and migration][migration], [storage][storage] | Use supported exports and source metadata. Document differences in stored auth, journal and synchronization state. Provide consistent verified backup, atomic restore, side-by-side comparison and rollback. | Require private backup storage and verify provenance, format and data integrity before restoring. | Restore on clean instance, kill during restore, migration count/query comparison and rollback rehearsal. | Open |
| Management REST API | Project extension. Upstream compatibility routes are tracked above. | Versioned API, validation and OpenAPI, paginated source and object controls. | RBAC, audit, request IDs and rate limits. Default administrative access restricted. | Contract and negative authorization tests, OpenAPI validation and abuse tests. | Open |
| Deployment | [Deployment manual][deploy]. Native packaging and multi-architecture artefacts are project requirements. | Provide Linux x86_64/ARM64 builds, Debian/RPM, OCI image, Compose and systemd configuration, backup and migration docs. | Non-root, controlled secrets, reproducible builds, SBOM and security audit. | Clean-machine install, graceful stop, upgrade/rollback, package and multi-architecture verification. | Open |
| Differential and performance qualification | [Pinned end-to-end tests][integration], [reference release][release] | Run both implementations on the same datasets and measured hardware under equivalent semantics and durability settings. Record resource use, latency distributions, throughput and variance. | Compare sanitized or public data, exercise abuse and failure modes and resolve high/critical vulnerabilities before release. | Results with exact configuration, dataset hashes, workloads, build and hardware details, reproducibility and long-run stability evidence. | Open |

[arch]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/development/architecture.rst
[storage]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/development/storage.rst
[lic]: https://github.com/irrdnet/irrd/blob/v4.5.4/LICENSE
[objects]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/rpsl/rpsl_objects.py
[parser]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/rpsl/parser.py
[models]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/storage/models.py
[whois]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/users/queries/whois.rst
[resolver]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/server/query_resolver.py
[pk-query]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/storage/queries.py#L67-L74
[graphql]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/users/queries/graphql.rst
[schema]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/server/graphql/schema_generator.py
[http]: https://github.com/irrdnet/irrd/blob/v4.5.4/irrd/server/http/app.py
[release]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/releases/4.5.4.rst
[mirroring]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/users/mirroring.rst
[config]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/configuration.rst
[event]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/users/queries/event-stream.rst
[changes]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/users/database-changes.rst
[validation]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/object-validation.rst
[suspension]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/suspension.rst
[ui]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/webui.rst
[rpki]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/rpki.rst
[scope]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/scopefilter.rst
[pref]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/route-object-preference.rst
[suppression]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/object-suppression.rst
[metrics]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/prometheus-metrics.rst
[cli]: https://github.com/irrdnet/irrd/blob/v4.5.4/pyproject.toml#L93
[migration]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/availability-and-migration.rst
[deploy]: https://github.com/irrdnet/irrd/blob/v4.5.4/docs/admins/deployment.rst
[integration]: https://github.com/irrdnet/irrd/tree/v4.5.4/irrd/integration_tests
[2622]: https://www.rfc-editor.org/rfc/rfc2622
[4012]: https://www.rfc-editor.org/rfc/rfc4012
[2725]: https://www.rfc-editor.org/rfc/rfc2725
[3912]: https://www.rfc-editor.org/rfc/rfc3912
[6811]: https://www.rfc-editor.org/rfc/rfc6811
[8481]: https://www.rfc-editor.org/rfc/rfc8481
[9582]: https://www.rfc-editor.org/rfc/rfc9582
[9580]: https://www.rfc-editor.org/rfc/rfc9580
[5322]: https://www.rfc-editor.org/rfc/rfc5322
[7464]: https://www.rfc-editor.org/rfc/rfc7464
[7515]: https://www.rfc-editor.org/rfc/rfc7515
[6455]: https://www.rfc-editor.org/rfc/rfc6455
[6238]: https://www.rfc-editor.org/rfc/rfc6238
[nrtm07]: https://datatracker.ietf.org/doc/draft-ietf-grow-nrtm-v4/07/
[nrtm-current]: https://datatracker.ietf.org/doc/draft-ietf-grow-nrtm-v4/
[ripe-nrtm]: https://docs.db.ripe.net/RIPE-Database-Mirror/Access-to-NRTM/
[gqlspec]: https://spec.graphql.org/
[webauthn]: https://www.w3.org/TR/webauthn-2/
