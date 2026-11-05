# Development status

RIRRd is beta software. The compatibility baseline is official IRRd 4.5.4 at
8e8080dab744a348b7d0e6efcb53f403301dd075. Full parity and release qualification
remain incomplete. RPKI validation and RTR are deliberately outside this project.

The implemented profile includes transactional embedded storage, bounded WHOIS,
HTTP and GraphQL, AS-set traversal, authoritative maintainer checks, NRTMv3
journaling/mirroring, signed one-shot NRTMv4 sync and verified backup/restore.
See the feature matrix for class, authentication, query and mirroring gaps.

Active changes go to `dev`. Main and release tags are kept stable. No packages
or releases are published from development. The self-hosted console remains
public and MIT licensed. Community/project websites and sponsored-node operations
are maintained in a separate private repository.

## Native public HTTP admission

Set `http_public_queries = true` on a loopback HTTP listener behind a trusted
reverse proxy. Disable HTTP authoritative updates. The proxy must overwrite
`X-RIRRd-Client` with its immediate client address and expose only GET
`/v1/whois/`, GET `/v1/irr/objects` and POST `/graphql/`. Never expose `/ui/*`.
Local monitoring without the forwarded header is excluded from the public budget.

The default policy remains two requests/second with a 20-request burst, grouped by IPv4
address or IPv6 /64. Client state is bounded to 4,096 entries; admitted HTTP work
is bounded to 16 requests plus the existing per-interface query budgets. Full
admission returns 503 with Retry-After; per-client exhaustion returns 429. It
never queues rejected clients behind query work. Prefix responses are bounded to 128 MiB (96 MiB query output plus JSON framing),
with a separate 256 MiB aggregate buffer budget. A single shared heavy-query lease
limits expansions beyond 262,144 prefixes through their final output consumption;
ordinary member and object-query budgets remain smaller. Admission and byte permits remain
attached to prepared response bytes until transport consumes or drops them; slow
consumers cannot accumulate unbounded completed replies. Raw object query bounds
remain separate. A response exceeding the configured cap returns 422 without a
partial answer. [Finite operator tuning and load checks](operations/HTTP_LOAD_TESTING.md)
separate public fairness from isolated capacity testing.

`/ui/http-stats.json` exposes aggregate counters on loopback only. Successful
handler p95 excludes rejected requests and does not include TLS/network delivery.
`Server-Timing` and an anonymous request sequence help distinguish native query
latency from front-proxy and network delay. No query-result cache is introduced.

## Query and update hardening — beta.4

Named source groups expand in order and deduplicate physical sources. They are
query-only: aliases cannot authorize updates, identify a mirror, or widen HTTP
source visibility. See [configuration and exact behavior](operations/SOURCE_ALIASES.md).

The development branch adds indexed GraphQL object filters (`mntBy`, `memberOf`,
`mbrsByRef`, `adminC`, `techC`, `zoneC`, `members`, `mpMembers`, `person`,
`role`, `origin`) and WHOIS `!j` journal-range discovery.
Inverse predicates intersect in one committed snapshot before applying the result
limit. Bounded index probes select the smallest candidate set rather than the
filter with the fewest input values, and scans check cancellation/deadlines. There is a 4,096-candidate work ceiling; exceeding it fails the field rather
than returning an incomplete intersection. See [API commands](operations/API_COMMANDS.md).

Public object lookups check a 2 MiB raw-text/identity budget before allocating
owned records. This is per storage lookup, not a whole-process or GraphQL-request
memory ceiling. Snapshot import can still retain 8 MiB records, and operator
exports preserve them. Authoritative submissions retain their 64 KiB record limit.
Set and object limits return explicit client errors without partial routing lists.

Each WHOIS request and complete GraphQL operation pins one committed database
snapshot. Mirror commits do not hold queries behind a global read lock, and all
GraphQL aliases see the same version. Mirroring and HTTP submissions acquire
their write-ordering guards asynchronously before scheduling mutations. Started
workers retain their snapshot or write guard after request cancellation; output
bodies retain only their memory/admission permits, not the database snapshot.
Deterministic regressions cover commit visibility, alias coherence and cancellation. Synchronous mirror network fetching can still occupy a blocking
worker until its timeout; a one-worker pool is not a low-latency mirror profile.

Authoritative contact creation now rejects handles already referenced by existing
objects, even without a deletion tombstone. Atomic deletion can remove an object
and all its referrers together; remaining or cross-source references still deny
the change. Beta.4 also adds authoritative `inetnum`, `inet6num` and `irt` validation and
authenticated workflows, checked through a shared upstream schema corpus and
normal CLI/HTTP entry points. These are specific authorization improvements, not
account or OpenPGP/key-cert parity. See [allocation/IRT evidence](compatibility/AUTHORITATIVE_ALLOCATIONS.md).

Verification groups already-sorted source/class counts without allocating group
keys per row. Public snapshot export sanitizes borrowed raw text instead of
cloning each entire object first. Neither change alters the on-disk schema or
removes data. The prior full-catalog repack remains the measured disk reduction.

## Release evidence

The [beta.4 qualification](compatibility/BETA4_QUALIFICATION.md) records the exact candidate, passing CI,
full-data checks, deployed HTTPS regression and measured performance trade-offs.

The [beta.4 release notes](releases/v0.1.0-beta.4.md) describe the current
candidate and required publication gates. The [query qualification](compatibility/QUERY_SOURCE_PARITY.md)
records the prior MVCC and large-query checks with exact commits and CI evidence. Tests on dev add rate/refill boundaries, IPv6 grouping, bounded client
state, saturated-worker rejection, response-size/deadline handling and normal
CLI/HTTP query checks. Read actual per-commit CI results rather than treating a
green subset as full parity.

Remaining work includes account/OpenPGP/token/mail flows, broader class/reference
and query behavior, NRTMv4 scheduling/publication/recovery, live scope and route
preference semantics, and sustained release qualification.
