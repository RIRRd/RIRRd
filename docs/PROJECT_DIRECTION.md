# Project direction

RIRRd is an MIT-licensed Rust implementation of an Internet Routing Registry
daemon. The goal is the feature set of a self-hosted IRRd service with less
operational and resource overhead. Full parity remains incomplete. The current
source and public issues are at [RIRRd/RIRRd](https://github.com/RIRRd/RIRRd).

## Product priorities

1. One daemon and embedded database, with explicit cache, thread, connection and
   input budgets. Keep the dashboard bundled and buildless, with local assets
   and no separate Node runtime or required external database/cache service.
2. Observable IRRd behavior within the IRR-only scope: authoritative workflows, mirroring, queries,
   policy and recovery. Smaller budgets must not silently weaken authentication,
   durability, source isolation or validation.
3. Straightforward BIRD/FRR integration through existing IRR tools. Document and
   test the route from RIRRd to bgpq4 to candidate router filters. Make source
   choice, freshness, failed generation and rollback visible.
4. Evidence for efficiency on constrained Linux hosts. Measure imports as well
   as steady queries. Report CPU, latency, process memory, cgroup memory, disk,
   timeouts and trade-offs. No production minimum-RAM claim from one fixture.
5. A useful read-only dashboard: object/prefix lookup, source and replication
   status, instance location/sponsor labels and setup guidance. Administration,
   accounts and submission UI require their own authentication/authorization work.
6. A community-supported network of independently operated, sponsored registry
   nodes with verified location, operator, source policy and sponsor information.
   No public nodes or sponsors are announced until confirmed and checked.

## Keep the interfaces distinct

The Rust service's dashboard is served on its existing loopback HTTP listener.
The public community website and node directory are
separate services at irrd.tools. rirrd.co.uk showcases the software and its
measured development evidence. Public registry deployment is authorized for
the experimental UK node, hosted by HYEHOST LLC.

IRR-derived routing filters and cryptographic RPKI origin validation are
complementary. RIRRd is not a replacement routing daemon or an RPKI validator.
BIRD/FRR consume generated policy; the filter generator queries RIRRd.

See [router integration](operations/ROUTER_INTEGRATION.md),
[small-host operation](operations/SMALL_HOSTS.md), and
[the community network plan](STATUS.md).
