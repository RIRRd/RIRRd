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
GraphQL aliases that individually fit the prefix budget must reject their combined
oversized result with `data: null`, then a valid origin query must recover with
its complete result. This tests a deliberate finite request budget, not upstream
acceptance parity, and brings the large suite to 61 assertions.

The beta.3 baseline above passed all 23 reference checks but failed 32 of the 36
candidate checks at existing query limits: 12 TCP, 11 HTTP and nine GraphQL cases.
Failures were explicit limit errors, not successful truncated replies. This
reproduces a real capacity difference with valid queries; small-fixture success
does not establish large-query parity. The fixture SHA-256 is
`cac8a073b2f5eedadbb27c93552af1bc4de1635cf20b81ab592631d59f5b150b`.

The subsequent development executable with SHA-256
`aa0445077a93508b2d4588004008ba9f2f8be87f2f7fd111f376a9fc7671a2f1`
passed all 325 source-scope assertions and all 61 large/budget/recovery assertions
on 5 November 2026. The run used the workflow's import/start/compare/stop sequence,
a dedicated candidate database, a 64 MiB configured database cache, eight TCP
connection slots and a five-second request timeout. It used no public forwarding
headers, so it does not qualify reverse-proxy admission or Internet transport.

| Serial mixed-query correctness run | Requests | Median | p95 |
| --- | ---: | ---: | ---: |
| IRRd TCP | 14 | 22.484 ms | 86.191 ms |
| RIRRd TCP | 14 | 17.922 ms | 135.022 ms |
| RIRRd HTTP WHOIS | 13 | 17.579 ms | 116.859 ms |
| IRRd GraphQL | 9 | 128.389 ms | 255.855 ms |
| RIRRd GraphQL | 9 | 70.843 ms | 216.963 ms |

These are different query shapes in a single correctness run on a shared
development host. They are not a sustained-load benchmark or a general speed
comparison. Each timing includes connection setup and response collection. p95
uses linear interpolation. The candidate's RSS increased from 10.89 MiB to
44.40 MiB, with a 44.40 MiB high-water mark, across the combined synthetic suites.
The aggregate GraphQL rejection took 218.093 ms; the following complete origin
query took 7.534 ms. These figures describe the synthetic fixture, not the full
public source database.

The implementation now streams indirect members, bounds direct lookup extraction
before allocating typed member vectors, shares a prefix/member budget across
GraphQL aliases, and checks cooperative deadlines/cancellation during large
origin, set and indirect-index scans. Blocking workers retain their concurrency
permits and coherent database view until they exit. Separate unit checks cover
scan cancellation and over-limit whole-response rejection. This does not remove
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
