# Use the public node with BIRD or FRR

The UK community node is available at **irrd.tools:43** over IPv4 and IPv6.
These examples query the public service. You do not need to run a local daemon
or use a local loopback address to query it.

## Find IPv4 and IPv6 prefixes

```sh
# IPv4 originated by an ASN
whois -h irrd.tools '!gAS3333'
# IPv6 originated by an ASN
whois -h irrd.tools '!6AS3333'
# Every ASN in an AS-set, including nested membership
whois -h irrd.tools '!iAS-RIPENCC,1'
# IPv4 prefixes from the AS-set
whois -h irrd.tools '!a4AS-RIPENCC'
# IPv6 prefixes from the AS-set
whois -h irrd.tools '!a6AS-RIPENCC'
```

Use the [interactive query tool](/dashboard/#query) to choose an example, change
its source and copy an equivalent HTTP command. Prefix expansion has service
limits. A timeout, error or rate limit must not be treated as an empty route list.

## Generate candidate BIRD prefix sets

Install `bgpq4` on your router-management host, then choose your actual peer ASN
or AS-set. The examples use RIPE NCC data for an observable public test.

```sh
bgpq4 -4 -b -h irrd.tools:43 -S RIPE -l IRR_IN4 AS-RIPENCC
bgpq4 -6 -b -h irrd.tools:43 -S RIPE -l IRR_IN6 AS-RIPENCC
```

Reference the generated definitions in your BIRD policy. Compare them with the
previous set and validate the complete candidate configuration with `bird -p -c`
before applying a reviewed configuration. Keep the last known good policy if a
query or validation step fails.

## Generate candidate FRR prefix lists

```sh
bgpq4 -4 -h irrd.tools:43 -S RIPE -l IRR_IN4 AS-RIPENCC
bgpq4 -6 -h irrd.tools:43 -S RIPE -l IRR_IN6 AS-RIPENCC
```

Attach the generated IPv4/IPv6 lists to your chosen inbound route-map. Use a
versioned candidate name, inspect additions and removals and validate against
your installed FRR version before changing an active policy. These commands
produce text; they do not modify the router.

## Keep router policy under your control

IRR filters use registered routing policy. Cryptographic origin validation is a
separate job for a dedicated RPKI validator; RIRRd does not provide RTR or ROA
validation. No community account is needed to run your own IRR instance.

Check [source health](/dashboard/#sources) before depending on a public node.
The service is experimental, with seven incremental feeds and twelve scheduled
snapshot sources at this checkpoint. Full query/policy parity is incomplete.
