# Query results and source selection

RIRRd and IRRd need the same source data and query settings before their answers
can be compared. Matching object totals alone does not establish equivalent
contents, current serials, source precedence or routing results. The additional
oracle corpus here checks those query semantics using synthetic data; it is not
a claim of complete IRRd compatibility or an audit of every live registry object.

## Conditions for an equivalent query

Use the same snapshot and applied source serial for every selected source, the
same retained object classes, and the same explicit source order. Record rejected
objects as well as accepted totals. An independent snapshot refresh and an NRTM
feed can legitimately be at different points in time. A stale or incomplete
source must not be described as equivalent just because its queries succeed.

Source order affects exact-key lookup and recursive set expansion. For example,
two sources may define `AS-EXAMPLE` differently. The first selected source supplies
that set definition; reversing the sources can therefore change its prefixes.
Origin queries combine applicable routes from the selected sources. These are
different operations, even when they start with the same ASN.
[Pinned WHOIS semantics](https://irrd.readthedocs.io/en/v4.5.4/users/queries/whois/).

Match recursion depth and exclusions, and compare complete prefix/member sets.
Do not treat an error, a truncated list or a subset of successful queries as a
successful complete comparison. RIRRd has explicit traversal, record and response
budgets; large legitimate upstream queries can currently exceed those budgets.
The synthetic corpus below fits within them and does not close that gap.

The current RIRRd beta is IRR-only. An IRRd reference that suppresses records using
RPKI, scope filtering or route preference can intentionally return different data.
Disable those policies for the unfiltered mirror/query comparison, or test each
policy separately with a documented expected difference. Router configuration
generation must retain its own validation and failure checks.

## Why a populated set can return an empty list

On 5 November 2026, a frozen copy of the hosted database contained an empty
`AS-HURRICANE` definition in ARIN and a different, populated definition in RADB.
The ARIN definition was 241 bytes with no members; the RADB definition was
282,180 bytes with 24,112 direct member entries. These are definition sizes and
direct member counts, not expanded ASN or prefix counts. ARIN preceded RADB in
that instance's configured source order. The default
`!a4AS-HURRICANE` and `!a6AS-HURRICANE` queries therefore returned empty lists
across TCP WHOIS, HTTP WHOIS and GraphQL. An empty list did not mean that the set
was absent from the database, or that its later RADB definition was missing.

The capture used ARIN serial `12012797` and RADB serial `6867732`; the query run
completed at `2026-11-05T12:25:26Z` with executable SHA-256
`aa0445077a93508b2d4588004008ba9f2f8be87f2f7fd111f376a9fc7671a2f1`.
The subsequent definition inspection did not record individual request times.
An explicit RADB direct-members HTTP query returned 200 with 204,779 bytes;
the default ARIN-first direct-members query returned 204 with an empty body.
These are observations from that frozen dataset, not promises about today's
upstream definitions. Matching answers across RIRRd transports does not establish
independent full-data equivalence with IRRd. The synthetic two-source oracle
corpus below provides the separate source-precedence comparison.

To inspect a particular source, select **RADB** in the console's **IRR source**
field. The equivalent persistent TCP WHOIS conversation is:

```text
!!
!sRADB
!iAS-HURRICANE,1
!q
```

For HTTP WHOIS, pass `q=!iAS-HURRICANE,1` and `sources=RADB` as URL-encoded
parameters to `/v1/whois/`. Use `!a4AS-HURRICANE` or `!a6AS-HURRICANE` to request
prefixes instead of recursive ASNs. Selecting only RADB also restricts the route
lookup to RADB; it does not merely change the root definition while retaining
routes from every other source. Origin queries such as `!gAS6939` and `!6AS6939`
combine applicable routes across the selected sources and do not consult an
AS-set definition.

To prefer RADB's set definitions while retaining route lookup across all 21
sources in the development profile, put RADB first and include the other sources:

```sh
curl --fail --get \
  --data-urlencode 'q=!a6AS-HURRICANE' \
  --data-urlencode 'sources=RADB,RIPE,ARIN,AFRINIC,LACNIC,APNIC,BELL,JPIRR,IDNIC,ALTDB,TC,REACH,LEVEL3,NTTCOM,RIPE-NONAUTH,CANARIE,BBOI,PANIX,NESTEGG,REGISTROBR,INTERNAL' \
  'https://dev.rirrd.net/v1/whois/'
```

This order applies to nested set definitions too. It preserves the route-source
scope, unlike `sources=RADB` alone. The example targets the **unreleased development
service and its larger finite query bounds**; the published beta.3 release retains
the earlier limits. Source availability and data change over time, so record the
selected order and source serials when comparing answers.

The bundled console displays its actual configured HTTP source order, explains
this distinction and treats an empty set response as an empty list rather than
proof of a missing object. Its browser regression uses the same set name with an
empty first-source definition and a populated second-source definition. It checks
explicit source selection, origin-prefix union, accessible explanatory text and
one WHOIS request per submit; no background source probing is added.

## Reproducible two-source corpus

[`query_scope_fixture.py`](../../tools/oracle/query_scope_fixture.py) generates
27 objects in each of `QUERY-FIRST` and `QUERY-SECOND`, covering all 19 standard
RPSL classes. The sources deliberately contain duplicate identities with different
AS-set definitions, nested and cyclic sets, authorized and unauthorized indirect
members, empty/missing sets, and differing IPv4/IPv6 origin routes. These are
non-strict mirror fixtures, not authoritative registration examples. No real
registry data or working credentials are included.

[`compare_query_scope.py`](../../tools/oracle/compare_query_scope.py) uses the
normal TCP WHOIS, HTTP WHOIS and GraphQL entry points. It selects both source
orders and each source individually. Every TCP response is checked for its byte
length and complete `A`/`C`/`D`/`F` framing before the values are compared.

On 5 November 2026, the beta.3 executable from
`b45a337042c39d8e07087b445ea7d283eb8ebd13` passed these checks against official
IRRd 4.5.4 at `8e8080dab744a348b7d0e6efcb53f403301dd075`:

| Scope | Assertions |
| --- | ---: |
| TCP results, source order and set exclusions | 100 |
| HTTP results, including explicit-source extension checked against upstream TCP | 100 |
| GraphQL routing and full-class object identities | 74 |
| Expected per-source, per-class fixture counts | 38 |
| Shared malformed-request rejection behavior | 11 |
| Documented stricter input bounds, intentionally different from upstream | 2 |
| Total | 325 |

The candidate executable SHA-256 was
`4269e11fd7dc0f0e20f742bab1c12bd48b8d22f7e3ca03e5cbc78f8204d405ca`.
The reference image ID was
`sha256:13fc702631c23e689a5530152dda869cf0c834e667be202662f673d064dc1f22`.
The fixture SHA-256 values are `acc8555941adeb187cb3005e1461e5f61749153bb4e71232d7269fd58a269f22`
and `3fdecae48f26f5f8a11956fcb77c3be50ff6e26a38fbd442dbca5d12d33d09f6`, respectively.

The existing 48 TCP and 48 HTTP WHOIS cases also passed separately with their
original `DIFFERENTIAL` source scope. The inverse GraphQL comparison passed its
12 results and six argument types. Adding test sources to default query selection
would change those older answers, so the new corpus uses a separate candidate
database and configuration in CI.

## Comparison allowances and known differences

- Unordered prefix, member and object collections are sorted **without removing
  duplicates**. Source identities and requested source order remain significant.
- Object attribute padding, comma-list spacing and the existing upstream auth
  placeholder omission allowance are normalized. Arbitrary descriptions are not.
- IRRd's HTTP WHOIS endpoint accepts `q`; RIRRd's `sources` parameter is an
  extension. Its result is checked against equivalent upstream TCP source
  selection. Shared HTTP tests use the standard `-s` query option on both servers.
- GraphQL `recursiveSetMembers` accepts a `rootSource` input in RIRRd as an
  extension. Upstream 4.5.4 has a `rootSource` output field but no matching input,
  so that input is excluded from this shared corpus.
- Malformed queries compare rejection behavior and HTTP status, not error prose.
  RIRRd rejects GraphQL ASN `4294967296` and negative recursion depth; the pinned
  upstream accepts them. These two cases are asserted and reported as differences.
- The extended large-query corpus records invalid GraphQL scalar child selections
  separately: upstream returns HTTP 400, while RIRRd returns HTTP 200 with GraphQL
  errors. Both reject the request with no data. Shared directive cases use Boolean
  JSON variables and literal ASNs; pinned upstream rejects ASN-list JSON variables,
  which RIRRd supports.

GraphQL class inventory requests include a maintainer filter because upstream
rejects a query consisting only of source and class as insufficiently specific.
[Pinned GraphQL semantics](https://irrd.readthedocs.io/en/v4.5.4/users/queries/graphql/).

## Large-query regression corpus

[`large_query_fixture.py`](../../tools/oracle/large_query_fixture.py) generates
27,562 synthetic objects in the separate `LARGE` source. Private ASNs and private
IPv4/IPv6 addresses are used throughout. Its `AS-LARGE` root contains 1,300 child
sets, a cycle and 5,200 origins. The complete answer has 10,400 IPv4 and 5,200 IPv6
prefixes; the combined WHOIS payload is 300,561 bytes. A separate origin has 5,200
prefixes in each family. Another set has 257 authorized indirect members and one
wrong-maintainer member that must remain excluded.

[`compare_large_queries.py`](../../tools/oracle/compare_large_queries.py) performs
59 shared-behavior checks: 23 reference and 36 candidate checks across TCP, HTTP and GraphQL.
Each successful answer must match the independently generated complete collection
with duplicates retained. The corpus includes recursive expansion, direct and
indirect membership, IPv4/IPv6/both-family prefixes, exclusions and missing roots.

Two additional candidate-only checks exercise the normal HTTP entry point: two
GraphQL root aliases, or two child aliases of `prefixes`, that individually fit
the prefix budget must reject their combined oversized result with `data: null`.
A third candidate-only check verifies that a valid origin query recovers with
its complete result. These test a deliberate finite request budget, not upstream
acceptance parity.

Another 14 shared checks cover distinct GraphQL row identities, child aliases,
fragments, Boolean variables, `@skip`/`@include`, reuse of the same alias in separate
requests, and invalid scalar child selections. The complete large suite now has
**76 assertions: 59 original comparisons, 14 selection/validation checks and three
candidate-only budget/recovery checks**. Validation HTTP status differences are
reported explicitly rather than normalized away.

The beta.3 baseline above passed all 23 reference checks but failed 32 of the 36
candidate checks at existing query limits: 12 TCP, 11 HTTP and nine GraphQL cases.
Failures were explicit limit errors, not successful truncated replies. This
reproduces a real capacity difference with valid queries; small-fixture success
does not establish large-query parity. The fixture SHA-256 is
`cac8a073b2f5eedadbb27c93552af1bc4de1635cf20b81ab592631d59f5b150b`.

The first complete large-query development qualification used executable SHA-256
`1c69f9435759dd9996a0346c5f9eccc46b8ce7e9652881c2d7c7f131010a92a2`
and passed **all 401 assertions: 325 source-scope plus 76 large-query checks**.
The run started at `2026-11-05T13:26:58Z` and took 10.411 seconds. Its dedicated
candidate database contained the same 27-object `QUERY-FIRST` and `QUERY-SECOND`
fixtures and 27,562-object `LARGE` fixture described above, with the recorded
fixture hashes unchanged. It used a 64 MiB configured database cache, eight TCP
connection slots and a five-second request timeout. This was a loopback comparison
against pinned IRRd 4.5.4 with isolated PostgreSQL/Redis. It used no public
forwarding headers, so it does not qualify reverse-proxy admission or Internet
transport. These measurements supersede the earlier 386-assertion development
run; the beta.3 failure results above remain the baseline.

| Serial mixed-query correctness run | Requests | Median | p95 |
| --- | ---: | ---: | ---: |
| IRRd TCP | 14 | 21.454 ms | 94.370 ms |
| RIRRd TCP | 14 | 11.913 ms | 117.599 ms |
| RIRRd HTTP WHOIS | 13 | 12.049 ms | 103.664 ms |
| IRRd GraphQL, original routing cases | 9 | 118.258 ms | 225.173 ms |
| RIRRd GraphQL, original routing cases | 9 | 76.032 ms | 189.616 ms |
| IRRd GraphQL, additional selection cases | 6 | 90.674 ms | 234.656 ms |
| RIRRd GraphQL, additional selection cases | 6 | 38.265 ms | 99.362 ms |

These are different query shapes in a single correctness run on a shared
development host. They are not a sustained-load benchmark or a general speed
comparison. Each timing includes connection setup and response collection. p95
uses linear interpolation. The candidate's RSS increased from 11.29 MiB to
76.23 MiB, with a 78.80 MiB high-water mark, across the combined synthetic suites.
The additional child-alias stress case operates on 166,400 prefixes before
duplicated output exceeds the shared budget; this is a larger test workload than
the earlier 386-assertion run. The final complete origin recovery query took
8.336 ms. These figures describe the synthetic fixture, not the full public
source database.

## Concurrent mirror updates

Development commit `bdf419b7a48c82af30d64ab28c0270a73ccdc109`, executable SHA-256
`5dec06b990a9d73eabb7bc6ce7cad3c387eaf0b4bf9f6f9e9cc38d205d732f7e`, replaces
the query-side writer gate with a committed redb snapshot for each request.
Writer and authentication ordering remain serialized. Recursive traversal and
all GraphQL aliases share one database view; completed bodies retain no database
snapshot, and canceled workers retain their view only until calculation ends.

Deterministic tests pause an actual NRTMv3 transaction between operations while
normal TCP and HTTP queries return the complete old state. After commit, fresh
requests see the replacement. A GraphQL test commits between aliases and checks
origins, recursive sets, object fields and source serials against the same old
view, followed by a fresh request seeing the new state. These are additional
consistency contracts; they do not establish upstream equivalence for every
possible concurrent schedule.

The exact executable passed all **401 oracle assertions** again at
`2026-11-05T14:40:03Z`, plus **324 core cases**, **49 browser cases** and
**87 operator/proxy cases**. Two optional diagnostic tests remain explicitly
skipped. The 39-check inactive full-data corpus returned the same complete answers
across TCP, HTTP WHOIS and GraphQL, including a 3,514,991-prefix set. Its
13,983,859 objects, all 21 source states and configuration were unchanged afterward.
Peak process RSS was 648.5 MiB during that correctness sequence under a three-CPU,
3 GiB service ceiling and 64 MiB database cache. The host filesystem cache was
warm; this is not a cold-cache throughput or minimum-host-memory qualification.

Passing CI for that source revision:
[core, including Linux x86-64, ARM64 and Windows](https://github.com/RIRRd/RIRRd/actions/runs/42047284498),
[pinned IRRd oracle](https://github.com/RIRRd/RIRRd/actions/runs/42047284602),
[router/browser integration](https://github.com/RIRRd/RIRRd/actions/runs/42047284739),
[deployment smoke](https://github.com/RIRRd/RIRRd/actions/runs/42047284459),
[fuzz smoke](https://github.com/RIRRd/RIRRd/actions/runs/42047284517) and
[source hygiene](https://github.com/RIRRd/RIRRd/actions/runs/42047284380).

The implementation now streams indirect members, bounds direct lookup extraction
before allocating typed member vectors, shares a prefix/member budget across
GraphQL aliases, and checks cooperative deadlines/cancellation during large
origin, set and indirect-index scans. Blocking workers retain their concurrency
permits and coherent database view until they exit. Separate unit checks cover
scan cancellation and over-limit whole-response rejection. GraphQL prefix lists
avoid allocating a resolver future for every scalar; per-request caches preserve
row identity, aliases and directives, and additional child aliases consume the
shared result budget. Typed resource/capacity/deadline errors discard all response
data. Large-query leases remain held through serialization and final HTTP byte
ownership, including slow readers. This does not remove
all finite query limits or complete the other IRRd parity gaps.

CI requires the full large-query corpus to pass and uploads `large-queries.json`
alongside the source-scope report. Per-request timings in that report are serial
correctness measurements including connection overhead, not throughput or
sustained-load qualification.

## Run and interpret the check

The [oracle workflow](../../.github/workflows/upstream-oracle.yml) contains the
complete isolated bootstrap and cleanup commands. It loads the same generated
files into both daemons and runs:

```sh
python3 tools/oracle/compare_query_scope.py --report /tmp/query-scope.json
python3 tools/oracle/compare_large_queries.py --report /tmp/large-queries.json
```

The workflow uploads the small JSON report as `source-query-parity`. It includes
the fixture hashes, assertion groups, normalization rules and known differences.
The configuration uses loopback listeners and dedicated PostgreSQL/Redis test
containers; it does not update public registries. A passing CI run demonstrates
the listed cases at that commit, not broader parity. Live cutover still requires
current source/serial coverage, complete real query results, recovery and load
checks appropriate to the deployment.
