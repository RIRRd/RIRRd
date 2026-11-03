# Embedded IRR and RPKI service

RIRRd can run the IRR database, query listeners, incremental mirror workers,
RPKI relying-party validation and an RTR server in one Rust process. RPKI is
optional: omit `[rpki]` for the smaller IRR-only profile. Project code is MIT;
the pinned **NLnet Labs Routinator 0.15.3** and rpki-rs libraries retain their
own licenses and attribution. This is an integration of their validator,
not an independently written replacement for its cryptographic implementation.

```toml
[rpki]
cache_dir = "/var/lib/rust-irrd/state/rpki"
rtr_listen = "127.0.0.1:3323"
refresh_seconds = 600
expire_seconds = 3600
validation_threads = 2
```

Use an absolute private cache directory writable by the daemon account. An
exclusive lock prevents two instances from updating the same cache. The
regular `config check` validates this profile; the normal `server` command
starts the validator. The cache survives daemon restarts and generation refresh.
Run only one process on it. The service account must own an existing cache after
an administrator's initial private validation run.

Validation uses all five bundled RIR trust anchors, authenticated HTTPS RRDP,
and upstream signature, certificate, manifest and CRL checks. Rsync is disabled
to keep collection inside the process. Rsync-only repositories are therefore
outside this initial profile. Rejected publication points are disclosed;
accepting all five anchors does not establish complete repository coverage.
The engine rejects payloads covered by unsafe resources, rather than ignoring
that protection to increase its count.

The first cached quick pass can be incomplete. In that case the beta immediately
falls back to a full validation pass instead of waiting the ordinary sixty-second
retry interval. Until a complete usable run publishes,
`GET /v1/rpki/validate` returns HTTP 503. A successful run atomically replaces
the origin-validation index and RTR history. Failed refreshes retain only a
still-fresh prior dataset. Expired or failed publication state is unavailable,
not a `NOT_FOUND` answer. RTR reset and serial queries return no-data errors
when the validated dataset is unavailable. Expiration sent to RTR clients is
bounded by remaining local freshness, including on repeat requests.

```sh
curl --fail http://127.0.0.1:9280/v1/rpki/status
curl --fail --get --data-urlencode 'prefix=193.0.0.0/21' \
  --data-urlencode 'origin=AS3333' http://127.0.0.1:9280/v1/rpki/validate
```

The live UK node exposes these bounded HTTP endpoints through HTTPS. Its RTR
listener is private loopback. For remote lab evaluation, an SSH port forward
from a router's management host can reach it:

```sh
# Substitute an actual authorized SSH account on the node; no account is implied.
ssh -N -L 3323:127.0.0.1:3323 USER@irrd.tools
```

Configure the lab RTR client on that management host to use localhost:3323.
The public node does not provide an anonymous SSH account. Operators running
their own RIRRd can place the application beside BIRD or FRR and use its local
RTR socket directly. RTR is not enabled on a public interface by this profile.
The [router integration guide](ROUTER_INTEGRATION.md) covers separate IRR filters.

A full global RPKI cache and cryptographic refresh need additional memory,
storage and CPU. IRR-only benchmark numbers must not be presented as the cost
of combined validation. Measure cold collection, steady refresh, peak cgroup
memory, cache bytes and failure recovery on the intended smaller host before
sizing it. Current resource telemetry includes the entire running process.

Remaining work includes rsync fallback or an explicit coverage policy, richer
per-repository observability, live IRR ROV suppression and journal transitions,
ASPA/BGPsec delivery policy, multi-router interoperability, long outage and
rollover qualification, and sustained resource measurements. The integration
is experimental and is not full Routinator or IRRd production parity.

## Beta API additions

The single-route `GET /v1/rpki/validate` response includes up to 32 covering ROAs,
ordered most-specific first, and an explicit truncation flag. The VALID/INVALID/
NOT_FOUND decision still checks every covering payload, including entries omitted
from the explanation. A covering ROA does not by itself authorise the route: ASN
and maximum prefix length must both match. ASN zero cannot authorise an origin.

Batch requests use the same endpoint with POST. A request pins one immutable
validation dataset and returns one serial/timestamp for all results. Empty batches,
more than 100 routes, malformed prefixes or origins fail the entire request.
Bodies are bounded to 16 KiB; unavailable/expired data returns 503.

```sh
curl --fail https://irrd.tools/v1/rpki/validate \
  -H 'Content-Type: application/json' \
  --data '{"routes":[{"prefix":"193.0.0.0/21","origin":"AS3333"},{"prefix":"2001:67c:2e8::/48","origin":"AS3333"}]}'
```

Status includes IPv4/IPv6 payload totals, dataset age, refresh/expiry intervals,
last validation duration and trust-anchor contribution/rejection counts. These
are observations, not a claim that rejected or rsync-only repositories were
included. The console plots contributed payloads per trust anchor. The ROV index
is constructed from typed verified prefixes, avoiding per-payload CIDR formatting
and reparsing on every refresh.
