# Upstream IRRd issue backlog

Snapshot taken **3 November 2026** against the public `irrdnet/irrd`
tracker. The [GitHub issues API][api] returned 40 unique open records
across pages of 15, 15 and 10. The response marked six records as pull
requests, leaving **34 open issues**. An empty label cell means GitHub
returned no labels. Classification and Rust scope are this project's
triage. For published IRRd 4.5.4 compatibility, use the separate
[feature matrix](IRRD_FEATURE_MATRIX.md).

To refresh, request `state=open&sort=created&direction=asc&per_page=15`
and follow GitHub's `Link: rel="next"` pagination. Verify unique issue
numbers, retain GitHub's titles and labels, split out records with the
`pull_request` field, and record the observation date. Compare releases,
public security advisories and merged fixes separately. A merged change
requires evidence before treating it as part of a tagged release.
[Issue tracker][tracker] · [API documentation][api-doc]

## Implementation priorities

### Security and correctness gates

| Upstream work | Rust acceptance criteria | Checkpoint |
| --- | --- | --- |
| [#190][i190] and [#561][i561] IPv4 canonical form and mandatory prefix length | Reject leading-zero IPv4 octets, bare IPv4 and IPv6 route addresses, wrong-family values and out-of-range lengths. Accept valid explicit CIDR lengths and preserve mirror rejection diagnostics. Cover import and authoritative validation separately. | The current mirror parser requires explicit CIDR lengths. It canonicalizes leading-zero IPv4 octets to match pinned 4.5.4 mirror behaviour. A strict write policy addressing #190 and authoritative validation remain open. |
| [#936][i936] folded AS-set membership | Single-line and folded comma-separated members produce the same tokens. Missing commas are rejected with a diagnostic on either form. Check continuation with spaces, tabs and `+`, inline comments, hierarchical names and weak references. | Open with RPSL attribute validation. |
| [#608][i608] per-client WHOIS connection cap | Make the per-peer limit configurable and combine it with the global cap. Canonicalize IPv4-mapped IPv6 peers, apply exemptions only to the per-peer cap, and release slots on disconnect, timeout and shutdown. Check that one saturated peer leaves capacity for another. | Partial. The [listener and admission tests](../../src/whois.rs) cover configurable caps, mapped addresses, slot release and capacity for a second peer. TCP session coverage includes an IPv4-mapped IPv6 exemption. Exempt clients still consume global capacity. Sustained abuse tests remain open. |
| [#877][i877] NRTMv3 serial discontinuity and [#1135][p1135] NRTMv4 session rollover | Replay duplicate and out-of-order entries, missing ranges and snapshot reseed while preserving last coherent objects, indexes and serials. For v4, retain rollback protection within a session and verify a legitimate signed new session through a new snapshot. Exercise forced reload and reject a replay of retired history. | Open with mirroring. The v3 thread includes several reported symptoms and does not establish a single root cause. |
| [#962][i962] ROA refresh with changing route inventory | Concurrent route add/replace/delete cannot abort a refresh due to stale identities or publish mixed ROA state. Inject a race in a controlled test and verify coherent retry, route status and journal transitions. | Open with RPKI. |
| [#1140][i1140] security report for 4.5.4 | Follow public advisory or release evidence and translate disclosed impact into a regression test. Keep public compatibility work separate from nonpublic vulnerability details. | Public report exists and upstream acknowledged it. No affected surfaces, reproduction or independent severity confirmation were public at capture. |

The four public advisories below describe already released fixes. Reproduce
the security properties in Rust even where the implementation differs.

| Advisory | Rust release evidence |
| --- | --- |
| [GHSA-pcwp-hhmm-c58f](https://github.com/irrdnet/irrd/security/advisories/GHSA-pcwp-hhmm-c58f) | Bound credential candidates, stored hash work factors, total verification work and concurrency. Exercise malicious legacy hashes and repeat submissions. |
| [GHSA-cvm7-qcww-5m59](https://github.com/irrdnet/irrd/security/advisories/GHSA-cvm7-qcww-5m59) | Reject or safely ignore malformed credential identifiers according to the submission contract. Verify that each error path releases transactions, permits and request state under repeated load. |
| [GHSA-22m3-c7vp-49fj](https://github.com/irrdnet/irrd/security/advisories/GHSA-22m3-c7vp-49fj) | Derive reset links from a configured origin and enforce trusted proxy rules. Verify that a forged Host or forwarded header cannot alter the link. |
| [GHSA-cqxx-66wh-8pjw](https://github.com/irrdnet/irrd/security/advisories/GHSA-cqxx-66wh-8pjw) | Check WHOIS, GraphQL, public exports, journals and events for credential leakage. Exercise auth values with continuations and mixed case. Keep raw operator export separately authorized. |

### Operator and protocol features

| Upstream work | Rust acceptance criteria | Checkpoint |
| --- | --- | --- |
| [#349][i349] persistent WHOIS integration coverage and [#329][i329] log assertions | Check an `!!` session, RIPE `-k` if supported, multiple framed successes and misses, session state isolation, idle timeout and cleanup. Capture child logs and fail on unexpected error events. | Partial. Real TCP tests cover pipelining, mixed success/error frames, source selection isolation and explicit close. RIPE keepalive parity, idle timeout and unexpected log assertions remain open. |
| [#952][i952] purge a retired source | Require an explicit destructive opt-in. Purge only the specified source's objects, indexes and source metadata atomically, verify other sources before and after reopening, and handle repeat and concurrent operations safely. Test physical space reclamation separately. | Partial. Offline purge requires --yes and clears the source objects, indexes, NRTMv3 serial and retained journal atomically. The CLI smoke test checks repeat purge, index integrity and source isolation after reopen. Separate offline compaction has local preservation tests and a measured synthetic run. Future metadata tables must join the same purge transaction. |
| [#886][i886] trigger export | Trigger a named source's export on demand with scheduler coordination, consistent data and companion serial, atomic publication and public redaction. Verify failure cleanup and concurrent export behavior. | Partial. Offline raw source export exists with an explicit sensitive-export acknowledgement. |
| [#917][i917] and [#1111][p1111] source-scoped GraphQL root | With the same set name in two sources, resolve the requested root source while applying the original allowed source order to descendants. Check invalid sources, alias behavior, mixed lists and preservation of existing GraphQL argument/coercion behavior. | Open with GraphQL. Preserve the capability without committing to the original delimiter syntax while upstream's input type proposal is open. |
| [#634][i634] limit journal query | Return the newest N matching authorized entries deterministically, bound N and preserve source and object filtering. | Open with GraphQL journals. |
| [#734][i734] reserved ASN validation | Define authoritative handling for AS0, AS23456, AS65535 and AS4294967295, with private-use ASNs kept valid where allowed. Test route, route6, aut-num and hierarchical AS-set keys. Keep mirror policy and AS0 ROA handling distinct. | Open. The report proposes validation policy beyond the pinned baseline. [RFC 7607](https://www.rfc-editor.org/rfc/rfc7607) · [RFC 6793](https://www.rfc-editor.org/rfc/rfc6793) · [RFC 7300](https://www.rfc-editor.org/rfc/rfc7300) |
| [#796][i796] incremental ROA updates | Apply a contiguous Routinator JSON delta atomically and compare against a full refresh. Cover resets, old serials, changed sessions, malformed records and validator outages. | Open with RPKI. |

## Open issue inventory

The Rust disposition records whether to implement observable behavior,
capture a regression, investigate a design, or account for an upstream
implementation detail. Overlap is noted where relevant. GitHub returned no
open issues with a duplicate label.

| ID | Exact upstream title | GitHub labels | Classification | Rust disposition |
| --- | --- | --- | --- | --- |
| [#7][i7] | Validate IRR object creation against the authoritative RIR database |  | Feature proposal | Define authoritative RIR integration and trust model. Related to delegation checks proposed in #734. |
| [#104][i104] | ease migration of irrd.conf to irrd4 config |  | Migration feature | Convert supported legacy irrd2/3 settings and report unknown, unsafe and lossy mappings. |
| [#190][i190] | invalid v4 route prefixes are being accept | enhancement | Validation bug | Reject ambiguous leading-zero IPv4 network octets. Related to #561, with separate inputs. |
| [#221][i221] | !a and key resolution failures |  | WHOIS feature | Provide bounded unresolved-member diagnostics while preserving !a success payloads. |
| [#279][i279] | Expose BGP validation in whois output | later | Feature proposal | Define trust, staleness and rendering rules for an opt-in BGP visibility feed. |
| [#328][i328] | Reduce IRRd warmup time by retaining Redis cache | later | Performance issue | Measure startup-to-query readiness and reuse indexes tied to committed generations. |
| [#329][i329] | Integration tests should check for errors in logs | later | Test improvement | Assert daemon logs contain no unexpected errors in integration tests. |
| [#349][i349] | Ensure multiple command mode is covered in tests | later | Test improvement | Cover persistent WHOIS command mode and state isolation end to end. |
| [#380][i380] | Config file examples: irr.net mirror list | later | Documentation feature | Supply verified, policy-cleared mirror examples and prerequisites. |
| [#389][i389] | Incorrect SQL database dependency in whois server tests | later | Upstream test cleanup | Keep protocol tests independent of PostgreSQL and retain embedded-storage integration tests. |
| [#412][i412] | Clean up updates parser tests | later, refactoring | Upstream test cleanup | Test validation rules independently and retain end-to-end multi-object coverage. |
| [#532][i532] | Refactor common components of scope filter / RPKI code | later, refactoring | Upstream refactor | Share suppression transitions and tests across scope filtering and RPKI. |
| [#561][i561] | IRRD incorrectly permits bare IP address for address-prefix | enhancement | Validation bug | Require explicit prefix lengths. Related to #190, with separate inputs. |
| [#570][i570] | Consider logging statistics for worker processes | enhancement | Observability feature | Measure connections, queries and load with bounded metric dimensions. |
| [#579][i579] | Separate scheduler and process manager | refactoring | Upstream refactor | Separate scheduling from execution and task lifecycle in the Rust worker design. |
| [#600][i600] | Refactor: improve mocking in tests | refactoring | Upstream test cleanup | Use deterministic test adapters and embedded-storage integration tests. |
| [#608][i608] | limit connections per source ip | enhancement | Security and availability feature | Add per-peer WHOIS connection bounds and exemptions alongside the global cap. |
| [#634][i634] | Add parameter to the "RPSLJournalEntry" resolver to limit the number of entries to the 'N' most recent entries | enhancement | GraphQL feature | Bound the number of recent journal results while preserving access checks. |
| [#690][i690] | Explore performance improvements in sync HTTP endpoints |  | Performance investigation | Benchmark Rust HTTP execution and streaming under equivalent work. The reported sync-wrapper behavior is Python-specific. |
| [#708][i708] | Various code improvements | later | Upstream maintenance | Cover analogous Rust formatting, lint and dependency checks. Python tooling changes are upstream-specific. |
| [#734][i734] | Refuse invalid origin ASNs in route objects |  | Validation policy proposal | Specify reserved-ASN validation and migration behavior. Separate delegated-ASN existence checks. |
| [#796][i796] | RPKI json delta import support |  | RPKI feature | Support validated Routinator delta replay with full-refresh fallback. |
| [#877][i877] | Possible issue in serial sequences on current RADB NRTM |  | Mirroring bug investigation | Test separate reported serial failure scenarios. The public thread leaves the root cause open. |
| [#886][i886] | provide external method to trigger exports |  | Operator feature | Extend local export to an on-demand scheduled operation for a named source. |
| [#907][i907] | UI WYSYIWIG |  | UI feature | Retrieve, edit, preview and save objects with authorization and optimistic concurrency. |
| [#917][i917] | Add support for "::" notation to GraphQL API |  | GraphQL feature | Pin the source of the root object while preserving descendant source selection. Related PR #1111. |
| [#936][i936] | as-set member validation behaves differently in single line and multiple line values |  | Validation bug | Validate comma-separated membership equivalently across folded and unfolded input. |
| [#952][i952] | Ability to remove/clean IRRd for objects with specific source | enhancement | Operator feature | Implement explicit transactional source purge and verify query and metadata state. |
| [#953][i953] | Ability to provide different sources for specific client or group of clients |  | Feature proposal | Define per-client defaults versus enforced visibility. Check aliases and recursive lookups for policy bypass. |
| [#962][i962] | KeyError exception during ROA import |  | RPKI bug report | Cover overlapping route mutation and ROA refresh with coherent publication. |
| [#995][i995] | Support for IP-resolvable docker service names in access lists |  | Deployment feature | Define DNS-backed ACLs, trusted resolution, refresh timing and fail-closed behavior. |
| [#1107][i1107] | Migrate to sqlalchemy 2 |  | Upstream dependency maintenance | Track the SQLAlchemy upgrade for the oracle. Rust has no SQLAlchemy dependency. |
| [#1117][i1117] | Usage with coredis >= 6.0, or switching away from coredis? |  | Upstream dependency question | Track coredis and limits compatibility for the oracle. Rust has no coredis dependency. |
| [#1140][i1140] | Vulnerabilities discovered while working with latest stable |  | Undisclosed security report | Await public advisory evidence to specify a regression. |

## Open pull requests in the same API snapshot

These changes are proposals or work in progress. Keep their verification
separate from the v4.5.4 tag.

| ID | Exact upstream title | GitHub labels | Relevance |
| --- | --- | --- | --- |
| [#723][p723] | [WIP] Ref #697 - Add profiling for whois and HTTP |  | Draft profiling tooling. Compare outcomes with the Rust benchmark harness. |
| [#967][p967] | irr_rpsl_submit: error message should be an f string |  | Report the offending argument safely in the Rust submission CLI when it exists. |
| [#1111][p1111] | Add support for registry scoped RPSL pk |  | Proposal related to #917. Treat schema changes as proposed until released. |
| [#1133][p1133] | Poll for mirroring and RPKI updates in the integration test |  | Use bounded polling for integration readiness and completion instead of fixed delays. |
| [#1135][p1135] | Fix NRTMv4 client refusing updates after a session change |  | Add authenticated NRTMv4 session rollover and forced-reload regression coverage. |
| [#1139][p1139] | Bump twisted from 26.4.0 to 26.10.0rc1 | dependencies, python | Upstream Python pre-release dependency update. Track for the oracle only. |

[tracker]: https://github.com/irrdnet/irrd/issues
[api]: https://api.github.com/repos/irrdnet/irrd/issues?state=open&sort=created&direction=asc&per_page=15
[api-doc]: https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api
[i7]: https://github.com/irrdnet/irrd/issues/7
[i104]: https://github.com/irrdnet/irrd/issues/104
[i190]: https://github.com/irrdnet/irrd/issues/190
[i221]: https://github.com/irrdnet/irrd/issues/221
[i279]: https://github.com/irrdnet/irrd/issues/279
[i328]: https://github.com/irrdnet/irrd/issues/328
[i329]: https://github.com/irrdnet/irrd/issues/329
[i349]: https://github.com/irrdnet/irrd/issues/349
[i380]: https://github.com/irrdnet/irrd/issues/380
[i389]: https://github.com/irrdnet/irrd/issues/389
[i412]: https://github.com/irrdnet/irrd/issues/412
[i532]: https://github.com/irrdnet/irrd/issues/532
[i561]: https://github.com/irrdnet/irrd/issues/561
[i570]: https://github.com/irrdnet/irrd/issues/570
[i579]: https://github.com/irrdnet/irrd/issues/579
[i600]: https://github.com/irrdnet/irrd/issues/600
[i608]: https://github.com/irrdnet/irrd/issues/608
[i634]: https://github.com/irrdnet/irrd/issues/634
[i690]: https://github.com/irrdnet/irrd/issues/690
[i708]: https://github.com/irrdnet/irrd/issues/708
[p723]: https://github.com/irrdnet/irrd/pull/723
[i734]: https://github.com/irrdnet/irrd/issues/734
[i796]: https://github.com/irrdnet/irrd/issues/796
[i877]: https://github.com/irrdnet/irrd/issues/877
[i886]: https://github.com/irrdnet/irrd/issues/886
[i907]: https://github.com/irrdnet/irrd/issues/907
[i917]: https://github.com/irrdnet/irrd/issues/917
[i936]: https://github.com/irrdnet/irrd/issues/936
[i952]: https://github.com/irrdnet/irrd/issues/952
[i953]: https://github.com/irrdnet/irrd/issues/953
[i962]: https://github.com/irrdnet/irrd/issues/962
[p967]: https://github.com/irrdnet/irrd/pull/967
[i995]: https://github.com/irrdnet/irrd/issues/995
[i1107]: https://github.com/irrdnet/irrd/issues/1107
[p1111]: https://github.com/irrdnet/irrd/pull/1111
[i1117]: https://github.com/irrdnet/irrd/issues/1117
[p1133]: https://github.com/irrdnet/irrd/pull/1133
[p1135]: https://github.com/irrdnet/irrd/pull/1135
[p1139]: https://github.com/irrdnet/irrd/pull/1139
[i1140]: https://github.com/irrdnet/irrd/issues/1140
