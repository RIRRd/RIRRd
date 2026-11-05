# Query source aliases and indexed object filters

These features are part of the beta.4 development candidate. Check the running
node's version and configuration before using a new alias; aliases are local
operator configuration, not names automatically shared between IRR servers.

## Name an ordered source selection

```toml
sources = ["RIPE", "RADB", "ARIN"]

[source_aliases]
ROUTING = ["RADB", "RIPE", "ARIN"]
EUROPE = ["RIPE", "RADB"]
```

Put `[source_aliases]` after the other top-level settings in a TOML file. The
physical `sources` list still controls the database and default source priority.
Aliases are query-only names: they do not create a source, copy any objects,
change mirror configuration, grant update authority or grant access to a hidden
HTTP source. Existing physical names continue to work.

Selection order is significant. `ROUTING,RIPE` expands to `RADB,RIPE,ARIN`, while
`RIPE,ROUTING` expands to `RIPE,RADB,ARIN`. Expansion keeps the first occurrence
of each physical source. A repeated logical alias remains visible in `!s-lc`,
matching the pinned IRRd selection behavior, but never repeats physical work.
The first matching set definition wins; prefix queries for an ASN combine
origins from all selected sources. An alias cannot reconcile different data or
mirror serials into one authoritative answer.

```sh
# The operator must first configure ROUTING on this example node.
whois -h irr.example.net -- '-s ROUTING AS-EXAMPLE'
printf '!!\n!sROUTING\n!a6AS-EXAMPLE\n!q\n' | nc irr.example.net 43
curl --fail --get --data-urlencode 'sources=ROUTING' \
  --data-urlencode 'q=!a6AS-EXAMPLE' https://irr.example.net/v1/whois/
curl --fail https://irr.example.net/graphql/ -H 'Content-Type: application/json' \
  --data '{"query":"{ asSetPrefixes(setNames:[\"AS-EXAMPLE\"],sources:[\"ROUTING\"],ipVersion:6) { rpslPk prefixes } }"}'
```

Alias names must be uppercase, start with a letter, end with a letter or digit,
and contain only letters, digits or hyphens, with length 2–32. At most 256 aliases
and 256 members per alias are accepted. Members must be configured physical
sources: unknown sources, nested aliases and name collisions fail `config check`.
A source-restricted HTTP interface hides an alias entirely if any member is
outside its allowed source list. It does not silently shorten the alias.

`databaseStatus` exposes aliases with `source_type: "alias"` and an ordered
`aliased_sources` list. Ask for those fields when listing aliases:

```graphql
{ databaseStatus { source source_type aliased_sources } }
```

Physical-source counters, serials and operational checks should explicitly
select physical source names. The pinned schema declares some physical status
fields non-null but omits them on alias rows; selecting fields such as
`authoritative` on an alias therefore produces a GraphQL error in both
implementations. Aliases are also not NRTM sources (`!jALIAS` reports an unknown
database), snapshot import/export targets, or source-status CLI identities.

## Bounded indexed filters

`rpslObjects` accepts `mntBy`, `memberOf`, `mbrsByRef`, `adminC`, `techC`, `zoneC`,
`members`, `mpMembers`, `person`, `role` and `origin`. Values are alternatives
within an argument; different arguments must all match. Filters can be combined
with `rpslPk`, `sources`, `objectClass` and `recordLimit` (0–8). Explicit empty
lists match nothing; null omits that filter. `origin` takes strings such as
`"AS64496"`, not integers. Source, class and complete object identity remain
separate, including identical routes in multiple sources.

```graphql
{
  rpslObjects(sources:["ROUTING"],origin:["AS64496"],mntBy:["EXAMPLE-MNT"],objectClass:["route6"],recordLimit:8) {
    rpslPk source objectClass objectText
  }
}
```

The existing inverse/origin indexes serve these queries without a database
migration. Cardinality probes choose a selective index before intersection,
within a shared 131,072-operation work ceiling. At most 4,096 candidate
identities may be examined; raw object materialization remains capped at 2 MiB.
Deadline and cancellation checks apply during probes, seeks and predicate reads.
An exhausted budget fails the query rather than returning a partial intersection.
The response limit is explicit pagination-free truncation to `recordLimit` for
object search; it is not a claim that every matching object was returned. Complete
routing-prefix queries use their separate all-or-error budgets.

## Pinned comparison and deliberate differences

The compatibility oracle is official IRRd 4.5.4 at
`8e8080dab744a348b7d0e6efcb53f403301dd075`. Repeatable synthetic comparisons live in
[`compare_source_aliases.py`](../../tools/oracle/compare_source_aliases.py),
[`query_scope_fixture.py`](../../tools/oracle/query_scope_fixture.py) and
[`query_lookup_fixture.py`](../../tools/oracle/query_lookup_fixture.py), alongside
normal CLI/TCP/HTTP scope tests in `tests/smoke.rs`. The initial development run
passed 158 alias/filter assertions, including the deliberate differences below;
this is a correctness corpus, not a throughput benchmark. The oracle workflow runs them
with isolated PostgreSQL and Redis, not against any public registry.

Two deliberate differences are tested explicitly:

* An empty alias stays empty in RIRRd on every query surface. In the pinned
  daemon, some object queries drop their source predicate after expanding an
  empty alias: `QUERY-EMPTY` can return an object from an unrelated source.
  RIRRd does not widen an empty selection this way. The same fail-closed rule
  applies to an explicitly empty indexed filter list: `members:[]` matches no
  objects, while the pinned daemon omits that predicate. RIRRd also treats an
  explicit null filter as omitted; pinned IRRd raises an internal resolver
  error for this nullable argument.
* RIRRd consistently normalizes indexed lookup values. Pinned IRRd's generic
  JSON lookup uppercases query values but leaves IPv6 `mp-members` and contact
  display names in their stored case. The synthetic `mpMembers` IPv6 query
  returns no reference rows but correctly matches `RS-QUERY` in both query
  sources in RIRRd. `person:["Query Fixture"]` and
  `role:["Query Fixture NOC"]` similarly match `QUERY1-TEST` and `QNOC1-TEST` in
  RIRRd. Separate uppercase-name fixtures verify shared positive behavior and
  argument types. This follows the upstream lookup code's stated
  case-insensitive contract instead of reproducing its inconsistent casing.

These comparisons establish the tested query behavior on identical fixtures.
They do not establish full IRRd parity, identical contents across live sources,
or support for the complete upstream GraphQL schema and policy model.
