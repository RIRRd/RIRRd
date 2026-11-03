# Real router-tool interoperability

The original synthetic [fixture](fixture.rpsl) contains 12 EXAMPLE objects and
reserved documentation resources. No public data or live update traffic is used.
The test uses the distribution bgpq4 client plus BIRD/FRR parser packages inside
an isolated Debian 13 image. Services are never installed on the VM host.

Build native RIRRd, import the fixture into a private EXAMPLE database and start
WHOIS on loopback (the [small-host guide](../../docs/operations/ROUTER_INTEGRATION.md)
has commands). Install the host's distribution `bgpq4` client, then build parsers:

```sh
docker build --network host -t rirrd-router-tools:local tools/router-integration
python3 tools/router-integration/check.py --candidate 127.0.0.1:4343
```

The test runs 12 real bgpq4 profiles: IPv4/IPv6 JSON, BIRD and IOS/FRR-style
prefix output, route-set recursion, exclusions, BIRD ASN sets and AS-path output.
It checks expected prefix sets, not only agreement between two empty results.
It also generates a candidate bundle, checks BIRD and FRR syntax with no network
or capabilities, and verifies that five failure cases leave the last good bundle
untouched. Router parser containers are capped at one CPU and 256 MiB.
No BGP session or live router configuration is changed by this harness.

For comparison with pinned official IRRd, build the existing
[oracle image](../oracle/README.md) as `irrd-454-oracle-reference`. Use a disposable
project and the dedicated overlay, which serves the same EXAMPLE fixture:

```sh
export RIRRD_REPO="$PWD"
export IRRD_ORACLE_PORT=8243 IRRD_ORACLE_HTTP_PORT=8280
docker compose -p rirrd-router-oracle -f tools/oracle/compose.yaml -f tools/router-integration/oracle.compose.yaml up --no-build -d --wait
python3 tools/router-integration/check.py --candidate 127.0.0.1:4343 --reference 127.0.0.1:8243
docker compose -p rirrd-router-oracle -f tools/oracle/compose.yaml -f tools/router-integration/oracle.compose.yaml stop
```

On the project's default-drop nftables VM, the isolated `irrd-router0` bridge
needs a temporary within-bridge forward rule. Preserve input/SSH rules and
remove only the temporary rule by its handle after testing. Containers need no
outbound runtime access; ports remain on 127.0.0.1.

The checked VM run used bgpq4 1.12, BIRD 2.17.5 and FRR 10.3. Twelve profiles
matched pinned IRRd 4.5.4. Parser and negative-generation checks passed.

`generate_filters.py` is a Linux staging tool. It requires an explicit endpoint,
source list, AS-set/ASN and new output directory. It bounds subprocess output
and time, accepts only nonempty exact-match lists, refuses default routes and
never reloads a router. `--allow-reserved-asns` is for synthetic tests or an
explicit operator policy, not a normal production default.
