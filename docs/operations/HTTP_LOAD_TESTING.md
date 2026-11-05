# Public HTTP admission and load testing

Public fairness and query capacity are different measurements. A workload that
receives mostly 429 replies exercises the client budget; it does not establish
how many complete queries the registry can execute. Evaluate accepted replies,
errors, recovery and resource usage separately. RIRRd remains beta software.

## Trusted proxy boundary

Native HTTP listens on loopback. With `http_public_queries = true`, the trusted
reverse proxy must **overwrite** `X-RIRRd-Client` using its immediate connection
address and expose only GET `/v1/whois/`, GET `/v1/irr/objects` and POST `/graphql/`.
Keep HTTP authoritative updates disabled. Never pass through a visitor's
`X-RIRRd-Client` value, trust arbitrary `X-Forwarded-For`/`Forwarded`, expose
`/ui/*`, or publish the native listener directly.

Requests without the proxy-owned header are local and excluded from public
admission, allowing local monitoring to remain responsive during saturation.
The dashboard and its exported telemetry need a separate, reviewed proxy route.
A private native-listener test verifies this contract; it does not validate a
particular deployment's proxy, DNS or TLS configuration.

## Finite configurable policy

Omitting the policy table preserves two requests/second and a 20-request burst,
grouped by IPv4 address or IPv6 /64. IPv4-mapped IPv6 uses the IPv4 budget. The
client table remains bounded to 4,096 entries. Idle entries expire only after
at least 60 seconds and enough time to refill their full configured burst.

```toml
http_public_queries = true

[http_public_policy]
rate_per_second = 2
burst = 20
max_active = 16
max_response_bytes = 20971520
max_response_buffer_bytes = 67108864
```

| Setting | Default | Accepted range / effect |
| --- | ---: | --- |
| `rate_per_second` | 2 | 1–10,000 accepted token attempts/second per client budget |
| `burst` | 20 | 1–10,000 tokens; admitted attempts consume a token even if capacity is busy |
| `max_active` | 16 | 1–16 public requests, including responses still retained by transport |
| `max_response_bytes` | 20 MiB | 1 KiB–32 MiB per complete serialized response |
| `max_response_buffer_bytes` | 64 MiB | At least `max_response_bytes`, at most 512 MiB across retained completed responses |

The old 2 MiB public-response cap could reject larger valid prefix results.
The new 20 MiB default accommodates the bounded 16 MiB prefix-query output and
JSON framing. Raw object materialization still has separate limits. The response
budget is not a total process-memory cap: in-progress query workers, temporary
serialization, database cache, sockets and runtime state consume additional RAM.
The eight-query handler/lookup ceilings are unchanged, and cancellation does not
stop a blocking worker already running. Its internal permit remains held until
the worker completes. Do not size RAM using only the response-buffer figure.

For a protected development capacity test, an explicit example is
`rate_per_second = 1000`, `burst = 2000`, with the same other bounds. This raises
the per-client budget without removing concurrency, response or work limits.
Run against synthetic private data first, record the settings and exact binary,
and choose rates appropriate to your host. Keep production fairness unchanged
unless its operator intentionally changes that policy. No setting promises
unlimited queries or raises the daemon's query-completeness limits.

## Responses and metrics

- **200:** a complete prepared response within the configured response budget.
  GraphQL also requires checking its `errors` array; HTTP 200 alone is not a
  successful GraphQL operation.
- **429 + Retry-After:** per-client token or client-table exhaustion.
- **503 + Retry-After:** admission, handler or aggregate response-buffer capacity
  is busy. Rejected work does not wait in a public admission queue.
- **422:** the serialized response exceeds the operator's public-response cap;
  no partial routing list is returned. Narrow the query or review the budget.
- **504:** request deadline reached. Inspect worker occupancy and query costs.
- **502:** a genuine response-body preparation error, not a rate-limit signal.

Loopback `/ui/http-stats.json` reports the effective policy and counters,
including `admission_busy`, `response_buffer_busy`, `response_limited`,
`response_errors`, `deadline_exceeded`, current/peak active requests and
current/peak `response_buffer_bytes`. Active requests include retained response
bytes, while successful latency covers handler/body preparation only, excluding
TLS and network delivery. Rejected attempts never lower successful percentiles.
Client cancellations before handler completion are counted separately; a worker
may continue after its caller times out. Counters reset when the daemon restarts.

## Repeatable synthetic regression

The standard core CI profile runs the native HTTP load contracts on every build:

```sh
cargo build --locked
python3 tools/testing/http_load.py \
  --binary target/debug/rust-irrd \
  --report /tmp/rirrd-http-load/cases
```

The suite starts only ephemeral loopback daemons; it accepts no remote target.
Its generated fixture contains 65,536 IPv6 prefixes, ensuring successful
WHOIS and GraphQL responses exceed 2 MiB. It checks every successful result in
full, not just status, under 32 simultaneous requests; allowed finite overload
is 503, with no unexpected 502, 504 or transport failures. It also checks
monitoring responsiveness, post-load recovery, malformed/duplicate proxy
headers, spoofed visitor forwarding headers, IPv6 /64 fairness, invalid budgets
and explicit response-limit failures. Rust tests additionally force saturation,
held slow-reader bodies, aggregate byte-budget exhaustion and disconnect cleanup.

The CI case count, result files and logs establish these exercised contracts.
They are not full IRRd parity, WAN reliability, or a maximum-throughput benchmark.
For capacity measurements use the same host, pinned binary/dataset and warmup,
report achieved starts/s, accepted complete responses/s, 429/503/other failures,
p50/p95/p99, RSS/PSS, cgroup memory and disk. Keep compilations, imports and other
benchmarks out of the measurement interval, and test recovery before increasing
load further.
