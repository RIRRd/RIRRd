# IRR API and command guide

Queries run against this RIRRd node's own database. The service is experimental,
read-only, and available over IPv4 and IPv6. A source can be a periodically
refreshed snapshot or a continuously followed mirror: check the
[live source status](https://rirrd.co.uk/dashboard/#sources) before using results.
IRR routing policy and cryptographic RPKI origin validation are complementary.

## IPv4 and IPv6 prefixes originated by an ASN

```sh
whois -h rirrd.co.uk '!gAS3333'
whois -h rirrd.co.uk '!6AS3333'
```

`!g` returns IPv4 prefixes and `!6` returns IPv6 prefixes. TCP responses use the
IRRd length framing. HTTP WHOIS returns the same payload without that framing.
The RIRRd `sources` HTTP parameter selects a source for this request:

```sh
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!gAS3333' https://rirrd.co.uk/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!6AS3333' https://rirrd.co.uk/v1/whois/
```

## All ASNs in an AS-set, direct members, and prefix expansion

```sh
# Recursively expand nested sets into ASNs.
whois -h rirrd.co.uk '!iAS-RIPENCC,1'
# Direct members can include nested set names.
whois -h rirrd.co.uk '!iAS-RIPENCC'
# Resolve recursive AS-set membership to originated IPv4 or IPv6 prefixes.
whois -h rirrd.co.uk '!a4AS-RIPENCC'
whois -h rirrd.co.uk '!a6AS-RIPENCC'

curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!iAS-RIPENCC,1' https://rirrd.co.uk/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!a4AS-RIPENCC' https://rirrd.co.uk/v1/whois/
curl --fail --get --data-urlencode 'sources=RIPE' --data-urlencode 'q=!a6AS-RIPENCC' https://rirrd.co.uk/v1/whois/
```

Results are deduplicated. Source order matters when the same set name occurs in
multiple sources. Use an explicit source or source list for repeatable results.
Expansion is bounded: depth 64, 1,024 set lookups, 4,096 origin lookups and
4,096 result items / 256 KiB. An over-limit query fails; a failure is not a
complete empty answer. Large transit sets can exceed these limits. Full IRRd
query parity remains incomplete.

## Inspect an object or exact route

```sh
whois -h rirrd.co.uk AS3333
whois -h rirrd.co.uk -- '-s RIPE -x 193.0.0.0/21'
curl --fail --get --data-urlencode 'q=-s RIPE AS-RIPENCC' https://rirrd.co.uk/v1/whois/
```

All loaded sources, including ARIN, use the public WHOIS, GraphQL and bounded
browsing interfaces. Queries are for Internet routing operations; results come
from this local RIRRd database.

## Structured GraphQL

```sh
curl --fail https://rirrd.co.uk/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ asnPrefixes(asns:[3333],ipVersion:4,sources:[\"RIPE\"]) { asn prefixes } }"}'
curl --fail https://rirrd.co.uk/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ recursiveSetMembers(setNames:[\"AS-RIPENCC\"],sources:[\"RIPE\"]) { rpslPk rootSource members } }"}'
curl --fail https://rirrd.co.uk/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ asSetPrefixes(setNames:[\"AS-RIPENCC\"],ipVersion:6,sources:[\"RIPE\"]) { rpslPk prefixes } }"}'
```

Check both the HTTP status and GraphQL `errors` field. The
[API workbench](https://rirrd.co.uk/dashboard/#api) introspects the actual schema.

## Browse AS-sets and routing classes

```sh
curl --fail --get --data-urlencode 'source=RIPE' --data-urlencode 'class=as-set' \
  --data-urlencode 'limit=30' https://rirrd.co.uk/v1/irr/objects
```

The response contains `keys` and `next_cursor`. Send that cursor as the `after`
parameter for the next page. Limits are 1–100 keys; pages use an index seek,
not a full database scan. Changes can occur between pages. Classes: `aut-num`,
`as-set`, `route-set`, `route`, `route6`, `filter-set`, `peering-set`, `rtr-set`,
`inet-rtr`.

## Live origin validation and telemetry

```sh
curl --fail https://rirrd.co.uk/v1/rpki/status
curl --fail --get --data-urlencode 'prefix=193.0.0.0/21' \
  --data-urlencode 'origin=AS3333' https://rirrd.co.uk/v1/rpki/validate
curl --fail https://rirrd.co.uk/dashboard/live.json
```

RPKI checks return `VALID`, `INVALID` or `NOT_FOUND` only with a usable locally
validated dataset. An unavailable or expired dataset returns HTTP 503, not
`NOT_FOUND`. The embedded validator uses Routinator's attributed Rust library
and RRDP; rsync fallback and live IRR suppression remain open. RTR is bound to
loopback; use the [router guide](/guides/routers/) for IRR filter generation.

Requests and results have bounded sizes, deadlines and shared rate limits.
Do not treat an HTTP error, timeout or rate limit as an empty routing policy.
