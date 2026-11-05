# IRR API and command guide

Queries run against this RIRRd node's own database. The service is experimental,
read-only, and available over IPv4 and IPv6. A source can be a periodically
refreshed snapshot or a continuously followed mirror: check the
[live source status](https://irrd.tools/dashboard/#sources) before using results.
IRR routing policy and cryptographic RPKI origin validation are complementary.

## IPv4 and IPv6 prefixes originated by an ASN

```sh
whois -h irrd.tools '!gAS3333'
whois -h irrd.tools '!6AS3333'
```

`!g` returns IPv4 prefixes and `!6` returns IPv6 prefixes. TCP responses use the
IRRd length framing. HTTP WHOIS returns the same payload without that framing.
The RIRRd `sources` HTTP parameter selects a source for this request:

```sh
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!gAS3333' https://irrd.tools/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!6AS3333' https://irrd.tools/v1/whois/
```

## All ASNs in an AS-set, direct members, and prefix expansion

```sh
# Recursively expand nested sets into ASNs.
whois -h irrd.tools '!iAS-RIPENCC,1'
# Direct members can include nested set names.
whois -h irrd.tools '!iAS-RIPENCC'
# Resolve recursive AS-set membership to originated IPv4 or IPv6 prefixes.
whois -h irrd.tools '!a4AS-RIPENCC'
whois -h irrd.tools '!a6AS-RIPENCC'

curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!iAS-RIPENCC,1' https://irrd.tools/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!a4AS-RIPENCC' https://irrd.tools/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!a6AS-RIPENCC' https://irrd.tools/v1/whois/
```

Results are deduplicated. Source order matters when the same set name occurs in
multiple sources. Use an explicit source or source list for repeatable results.
The beta.4 candidate also supports operator-defined [ordered source aliases](SOURCE_ALIASES.md);
alias names must be configured on the node before use.
Beta.3 deployments use depth 64, 1,024 set lookups, 4,096 origin lookups and
4,096 result items / 256 KiB. The beta.4 candidate supports 65,536 set lookups,
131,072 origin lookups and 2 million scanned rows, with up to 4 million AS-set
prefixes / 96 MiB; ASN-prefix and member lists retain separate 262,144-item
limits. Check the node version before relying on these expanded bounds. An
over-limit query fails; a failure is not a complete empty answer. Full IRRd
query parity remains incomplete.

## Inspect an object or exact route

```sh
whois -h irrd.tools AS3333
whois -h irrd.tools -- '-s RIPE -x 193.0.0.0/21'
curl --fail --get --data-urlencode 'q=-s RIPE AS-RIPENCC' https://irrd.tools/v1/whois/
```

All loaded sources, including ARIN, use the public WHOIS, GraphQL and bounded
browsing interfaces. Queries are for Internet routing operations; results come
from this local RIRRd database.

## Structured GraphQL

```sh
curl --fail https://irrd.tools/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ asnPrefixes(asns:[3333],ipVersion:4,sources:[\"RIPE\"]) { asn prefixes } }"}'
curl --fail https://irrd.tools/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ recursiveSetMembers(setNames:[\"AS-RIPENCC\"],sources:[\"RIPE\"]) { rpslPk rootSource members } }"}'
curl --fail https://irrd.tools/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ asSetPrefixes(setNames:[\"AS-RIPENCC\"],ipVersion:6,sources:[\"RIPE\"]) { rpslPk prefixes } }"}'
```

Check both the HTTP status and GraphQL `errors` field. The
[API workbench](https://irrd.tools/dashboard/#api) introspects the actual schema.

## Browse object classes

Development also supports indexed GraphQL object filters. Replace the example
maintainer with the name you want to query:

```sh
curl --fail https://dev.rirrd.net/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ rpslObjects(mntBy:[\"EXAMPLE-MNT\"],sources:[\"RIPE\"],objectClass:[\"as-set\"],recordLimit:8) { rpslPk source objectText } }"}'
```

Supported inverse filters in the beta.4 candidate are `mntBy`, `memberOf`,
`mbrsByRef`, `adminC`, `techC`, `zoneC`, `members`, `mpMembers`, `person`, `role`
and `origin`. Values within a filter are alternatives; different filters must
all match. They can be combined with `rpslPk`, source and class filters. Reads
use one database snapshot and existing indexes, and return at most eight objects.
Bounded index-cardinality probes choose a selective filter first. Intersections
that inspect over 4,096 candidates or exhaust their shared work budget fail as a whole;
narrow the keys, sources or classes instead of treating that error as no match.
An explicitly empty filter matches nothing; null means no filter. This bounded
profile does not provide the complete upstream GraphQL filter schema.

Development object queries check a 2 MiB raw text/identity budget before copying
records. An oversized query returns a limit error, never a partial routing list.
Larger mirrored objects remain in storage and can be exported by the operator.
The key browsing endpoint below can list their identities without loading text.

```sh
curl --fail --get --data-urlencode 'source=RIPE' --data-urlencode 'class=as-set' \
  --data-urlencode 'limit=30' https://irrd.tools/v1/irr/objects
```

The response contains `keys` and `next_cursor`. Send that cursor as the `after`
parameter for the next page. Limits are 1–100 keys; pages use an index seek,
not a full database scan. Changes can occur between pages. Current development
builds accept all 19 [supported object classes](SOURCE_PROFILES.md), including
contacts, allocation records, maintainers and public key certificates. A node's
import profile determines which records it actually retains; the older hosted
production profile contains routing classes only.

On a full-profile development node, browse maintainer keys and inspect a selected
object using a typed lookup. Authentication fields are redacted:

```sh
curl --fail --get --data-urlencode 'source=RIPE' --data-urlencode 'class=mntner' \
  --data-urlencode 'limit=30' https://dev.rirrd.net/v1/irr/objects
# Replace EXAMPLE-MNT with a returned key.
curl --fail --get --data-urlencode 'sources=RIPE' \
  --data-urlencode 'q=!mmntner,EXAMPLE-MNT' https://dev.rirrd.net/v1/whois/
```

The same `!mCLASS,KEY` lookup selects person/role records by nic-hdl and keeps
route origins in composite keys. Browse is read-only and source-scoped.

## Live source and resource telemetry

```sh
curl --fail https://irrd.tools/dashboard/live.json
curl --fail https://irrd.tools/dashboard/history.json
```

These endpoints expose aggregate source health, object counts and observed node
resource usage, not query contents or client identities. Historical samples have
explicit gaps when no measurement is available. RIRRd provides IRR queries only;
there are no RPKI validation endpoints or RTR listener.

Requests and results have bounded sizes, deadlines and shared rate limits.
Do not treat an HTTP error, timeout or rate limit as an empty routing policy.
