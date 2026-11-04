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

## Native public HTTP admission (development)

Set `http_public_queries = true` on a loopback HTTP listener behind a trusted
reverse proxy. Disable HTTP authoritative updates. The proxy must overwrite
`X-RIRRd-Client` with its immediate client address and expose only GET
`/v1/whois/`, GET `/v1/irr/objects` and POST `/graphql/`. Never expose `/ui/*`.
Local monitoring without the forwarded header is excluded from the public budget.

The policy remains two requests/second with a 20-request burst, grouped by IPv4
address or IPv6 /64. Client state is bounded to 4,096 entries; admitted HTTP work
is bounded to 16 requests plus the existing per-interface query budgets. Full
admission returns 503 with Retry-After; per-client exhaustion returns 429. It
never queues rejected clients behind query work. Responses are bounded to 2 MiB.

`/ui/http-stats.json` exposes aggregate counters on loopback only. Successful
handler p95 excludes rejected requests and does not include TLS/network delivery.
`Server-Timing` and an anonymous request sequence help distinguish native query
latency from front-proxy and network delay. No query-result cache is introduced.

## Evidence

[Beta.2 qualification](https://github.com/RIRRd/RIRRd/actions/runs/41813943576)
passed all 11 release jobs. The release was qualified before the current dev
work. Tests on dev add rate/refill boundaries, IPv6 grouping, bounded client
state, saturated-worker rejection, response-size/deadline handling and normal
CLI/HTTP query checks. Read actual per-commit CI results rather than treating a
green subset as full parity.

Remaining work includes account/OpenPGP/token/mail flows, broader class/reference
and query behavior, NRTMv4 scheduling/publication/recovery, live scope and route
preference semantics, and sustained release qualification.
