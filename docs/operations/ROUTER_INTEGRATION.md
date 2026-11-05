# Use RIRRd with BIRD and FRRouting

RIRRd supplies IRR data. A filter generator such as **bgpq4** queries it and
produces routing policy that BIRD or FRR can consume. Changing an IRR endpoint is
normally a change to the generator's `-h` argument and its explicit `-S` source
selection, followed by validation of the candidate router configuration.

This is a development integration guide. The experimental public test service uses
`irrd.tools:43` for read-only TCP WHOIS and bgpq4, and
https://irrd.tools/dashboard/ for live queries/status. Check the source list and
snapshot timestamps there first. Full parity, live policy filtering and production
qualification remain incomplete. Keep last-known-good filters and review candidates.

For a public test using RIPE's real AS3333 records:

```sh
whois -h irrd.tools -- '-s RIPE AS3333'
bgpq4 -4 -b -h irrd.tools:43 -S RIPE -l RIPE_TEST AS3333
bgpq4 -6 -b -h irrd.tools:43 -S RIPE -l RIPE_TEST6 AS3333
```

Generate a new BIRD/FRR candidate bundle using the same guarded helper below:

```sh
python3 tools/router-integration/generate_filters.py \
  --endpoint irrd.tools:43 --sources RIPE --object AS3333 \
  --name RIPE_TEST --output "$HOME/rirrd-public-candidate-001"
```

These commands generate candidate text, not router changes. The example ASN is
RIPE NCC's; substitute your actual peer/customer ASN or AS-set when building your
own policy. Do not permit an example ASN on a real customer session. All loaded sources, including ARIN, are available through the public query interfaces.
The following synthetic workflow also works entirely on your own private instance.

## 1. Start a private evaluation instance

Build the daemon, then use the small-host profile and synthetic fixture:

```sh
cargo build --locked --release
rirrd_eval=$(mktemp -d "$HOME/rirrd-evaluation.XXXXXX")
cp examples/small-host/irrd.toml "$rirrd_eval/irrd.toml"
target/release/rust-irrd --config "$rirrd_eval/irrd.toml" import snapshot --source EXAMPLE --file tools/router-integration/fixture.rpsl
target/release/rust-irrd --config "$rirrd_eval/irrd.toml" server
```

The commands create a new private evaluation directory. Keep its database private. Leave the daemon running and use another terminal for the following queries. The profile
binds WHOIS to `127.0.0.1:4343` and HTTP/dashboard to `127.0.0.1:8080/ui/`.
The database path is relative to the config file. These commands replace only
the evaluation source in that separate database.

For access from your workstation, tunnel to your own host rather than exposing
development ports. Replace `operator@your-host` with your SSH identity:

```sh
ssh -N -L 4343:127.0.0.1:4343 -L 8080:127.0.0.1:8080 operator@your-host
```

## 2. Query it with bgpq4

On Debian 13, install the distribution's `bgpq4` package. The checked fixture
contains documentation ASNs and prefixes. Its reserved ASNs require `-p`:

```sh
bgpq4 -p -4 -b -h 127.0.0.1:4343 -S EXAMPLE -l DOCS_IN4 AS64496:AS-DOCS
bgpq4 -p -6 -b -h 127.0.0.1:4343 -S EXAMPLE -l DOCS_IN6 AS64496:AS-DOCS
```

Expected IPv4 prefixes are `192.0.2.0/24` and `198.51.100.0/24`.
Expected IPv6 prefixes are `2001:db8:100::/48` and `2001:db8:200::/48`.
These are synthetic resources; do not announce them to the Internet.

For a real peer, substitute its ASN/AS-set and your intended sources. Omit `-p`
unless your policy deliberately accepts reserved/private ASNs. Do not keep
`EXAMPLE` or an assumed all-sources default in a production generator.

The same generator speaks to official IRRd and RIRRd. The checked-in integration
harness compares 12 real bgpq4 profiles, including both families, recursive AS-set
and route-set expansion, exclusions, BIRD/IOS-style output and ASN/path output.

## 3. Generate a guarded candidate bundle

The helper creates both BIRD definitions and FRR exact-match prefix lists from
the same per-family bgpq4 JSON results:

```sh
python3 tools/router-integration/generate_filters.py \
  --endpoint 127.0.0.1:4343 --sources EXAMPLE \
  --object AS64496:AS-DOCS --name DOCS_IRR --allow-reserved-asns \
  --output "$HOME/rirrd-candidate-001"
```

The output path must be new, and its parent must exist. The bundle includes
`filters.bird`, `filters.frr`, `prefixes.json` and a timestamped, hashed manifest.
It never connects to router management or reloads a router. Successful inputs
are staged before the complete candidate directory is published.

Failed queries, unknown sources, empty selected families, default routes,
non-exact entries and over-budget outputs fail the run. Existing bundles are
never overwritten. `--families 4` or `--families 6` explicitly selects a
single-family peer; the default expects nonempty results for both families.
The default candidate budget is 4,096 prefixes per family, configurable up to
65,536 with `--max-prefixes`. This helper does not aggregate or broaden matches.

## 4. BIRD: include and validate

`filters.bird` contains complete `define DOCS_IRR_4 = [...]` and
`define DOCS_IRR_6 = [...]` statements. The generator's raw `bgpq4 -b` output
alone is not the full include used by this guide.

[The BIRD example](../../examples/routers/bird.conf) shows inbound filters and an
administratively disabled synthetic peer. Its intended installed include is
`/etc/bird/rirrd/filters.bird`. Adapt the full candidate file and make the include
readable by the BIRD service account, then validate as that account or an
appropriately privileged operator:

```sh
bird -p -c /path/to/complete-candidate-bird.conf
```

Inspect the complete diff and retain the previous include/configuration. Apply
using your existing BIRD operational process only after validation. Re-evaluate
existing routes according to your version's reconfiguration/route-refresh
behavior; a soft filter edit alone is not proof that previously received routes
were rechecked. No live router is changed by this repository's generator/tests.

## 5. FRR: prefix lists and inbound route-maps

`filters.frr` uses separate `ip prefix-list` and `ipv6 prefix-list` names. The
[FRR example](../../examples/routers/frr.conf) attaches them to inbound route-maps
and leaves the synthetic peer shut down. Prepend the generated definitions to a
complete candidate configuration, then check it with the version you deploy:

```sh
/usr/lib/frr/bgpd -C -f /path/to/complete-candidate-frr.conf
```

The isolated test uses `-S` to skip privilege switching in its capability-free
container; that flag is not a production-service recommendation.

For updates, choose a new versioned list name and switch the reviewed route-map
reference. Appending regenerated entries to an existing list can leave stale
permits behind. Keep the previous list/configuration for rollback and remove it
only when no policy references it. Apply and refresh inbound policy through
your existing FRR procedure; do not blindly replace an entire `frr.conf` with
this small example.

## Failure handling and policy scope

A failed or unexpectedly empty generation must not replace the last good
policy. Alert on the failed refresh and its age, review large prefix-count
drops and define an operator policy for maximum acceptable staleness. Keeping
old filters indefinitely without alerting is not a freshness guarantee.

These examples provide exact-prefix IRR filtering, not a complete routing
security policy. Preserve your bogon/default-route controls, peer/AS-path rules,
limits and independent RPKI origin-validation policy. An IRR prefix list does
not cryptographically prove ownership or bind every received path to a valid
origin. RIRRd is not an RPKI-RTR cache or a BIRD/FRR replacement.

## Evidence and references

The local checkpoint exercised bgpq4 1.12, BIRD 2.17.5 and FRR 10.3. Both router
configuration parsers ran with no network, no Linux capabilities, one CPU and
256 MiB memory. These checks validate generated syntax and client interoperability;
they do not establish live BGP convergence or complete production policy parity.

- [bgpq4 official options and source selection](https://github.com/bgp/bgpq4)
- [BIRD 2 configuration and reconfiguration](https://bird.nic.cz/doc/bird-2.17.1.html)
- [FRR prefix lists](https://docs.frrouting.org/en/stable-10.3/filter.html)
- [FRR BGP policy and route refresh](https://docs.frrouting.org/en/stable-10.3/bgp.html)
- [Run the isolated integration checks](../../tools/router-integration/README.md)
