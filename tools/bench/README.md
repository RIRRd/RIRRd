# Synthetic scale comparison

This optional, root-only controller uses a **disposable** `rirrd-scale` Docker
project on the dedicated 24-CPU Linux VM. It deletes that project's PostgreSQL
volume between imports. Never point it at production data or a shared project.
It preserves the public website and uses only loopback registry listeners.

Generate the synthetic fixture outside Git, using an empty private state directory:

```sh
install -d -m 700 /var/lib/rirrd-dev/scale
python3 tools/bench/generate_fixture.py /var/lib/rirrd-dev/scale/synthetic-100k.rpsl > /var/lib/rirrd-dev/scale/fixture.json
cp tools/bench/irrd.scale.yaml /var/lib/rirrd-dev/scale/irrd.yaml
cp tools/bench/compose.scale.yaml /var/lib/rirrd-dev/scale/compose.yaml
cargo build --locked --release --example registry_load
cargo build --locked --release
```

Build/tag the pinned oracle image as `irrd-454-oracle-reference` using
[the oracle instructions](../oracle/README.md). The expected image in the
November 2026 VM checkpoint is recorded in the comparison JSON. The reference
configuration has eight WHOIS and eight HTTP workers and RPKI disabled. PostgreSQL
and Redis remain isolated; Redis persistence is disabled. No real messages or
updates are sent to external registries.

Stop any other oracle on 8043/8000 and candidate on 9043/9080 before running.
Ensure the host's forward policy permits traffic **within** `irrd-scale0` only;
this benchmark does not need public registry ports or outbound container access.
On the project's nftables host, one temporary rule was used:

```sh
nft 'add rule inet rirrd_host forward iifname "irrd-scale0" oifname "irrd-scale0" accept comment "RIRRd scale benchmark"'
python3 tools/bench/run_scale.py --state /var/lib/rirrd-dev/scale --candidate "$PWD/target/release/rust-irrd" --candidate-commit "$(git rev-parse HEAD)" --seconds 20
```

Remove that specific rule by its handle afterward. Do not change input/SSH rules.
The controller stops its services in `finally`, retaining only the final private
volume and files. It writes per-command logs and incremental `results.json` to a
unique state subdirectory. A comparison is complete only with `completed_at` and
zero errors in all profiles. A failed profile's log retains its error counts;
do not present it as a successful result.

Optional `--before /absolute/path/to/previous-binary` adds three alternating
10-second GraphQL identity-only runs for each binary on the same database.
The checked-in site renderer currently expects this optimization comparison.
Record the previous binary's source revision separately; its SHA is automatic.

## Scope

- Three fresh imports, alternating order; warm host file cache is possible.
  CLI wall time, CPU and peak CLI RSS are recorded. PostgreSQL schema migration
  and service startup are outside import timing. Both imports must accept exactly
  100,000 objects. CLI peak RSS excludes separate database/cache processes.
- Servers, PostgreSQL and Redis use CPUs 0–15; the native validating client uses
  CPUs 16–23. Eight concurrent clients issue closed-loop requests against one
  implementation at a time. No build/fuzz jobs run during timing.
- Three 20-second runs per reused-connection WHOIS/GraphQL workload, alternating
  order, after three-second warmups. One additional 10-second run per mode and
  implementation opens a new connection for each request.
- Both select from the same 50,000-ASN deterministic key space. WHOIS validates
  length/trailer and identity/source; GraphQL validates one exact identity/source/
  class and rejects errors. It requests just three common identity fields.
- Reused connections reconnect after 900 requests to remain under the candidate's
  1,024-request session bound. Candidate connection cap is 32, allowing reconnect
  overlap; active query concurrency remains eight. Individual requests time out
  at five seconds. Errors invalidate the profile. Warmup errors are excluded
  from measured counts. Every measured attempt is included in latency samples.
- Whole-stack idle memory is median summed process PSS across three samples,
  five seconds after the final query. Resident mmap pages count; unmapped page
  cache does not. Allocated storage is PostgreSQL cluster plus WAL versus redb;
  container images, logs and Redis persistence are excluded.
- Driver CPU is recorded against its eight allocated logical CPUs. Inspect it
  before interpreting throughput as server saturation. This is a local short
  synthetic checkpoint, not the full sustained/mixed-data qualification in
  [the methodology](../../docs/benchmarks/METHODOLOGY.md).

Fixtures, logs, databases and caches stay outside Git. Commit the generator,
controller, configuration and sanitized measurement JSON only. The static site
on `codex/coming-soon` renders the completed JSON via
`python3 site/coming-soon/render.py /path/to/results.json` before deployment.

If a client defect aborts the query stage **after all three valid imports**,
correct and rebuild the client, then use `--resume /absolute/state/measured-...`.
The controller checks binary/fixture identity and import counts, archives the
partial attempt privately and reruns every query profile. It retains the fresh
imports and explicitly records why the VM checkpoint was resumed. Do not resume
across server/configuration/data changes; generate a new run instead. The November
checkpoint corrected a rejected HTTP Host header; no failed profiles were used
as successful rates.

RIRRd and IRRd have different validation and index coverage. Equal accepted
counts and matching selected query fields do not establish equal total work or
full behavior parity. Report these as measurements of the configured stacks,
not an isolated language comparison.

Resume also requires `--resume-reason 'brief public explanation'`. The upstream
allowlist diagnosis was verified directly: the same GraphQL POST returned 400
with `Host: localhost` and 200 with the configured endpoint Host. The corrected
client passed a live request preflight before restarting the query suite.

## ROV and monitoring delivery

`compare_rpki_index.py` compares a pinned RIRRd source revision with the current
ROV kernel using synthetic prevalidated payloads; CPU and wall time are recorded.
`compare_history_gateway.py` exercises the normal history HTTP handler in isolated
processes. Both require an explicit work/output location outside Git. See
[the efficiency checkpoint](../../docs/benchmarks/UI_EFFICIENCY.md) for fixture,
measurement scope, exact revisions and raw results. These do not run a public
load test or replace the separate official IRRd oracle comparison.
