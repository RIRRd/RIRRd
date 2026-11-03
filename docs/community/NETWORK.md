# Community IRR network — first node

The intended service combines MIT-licensed registry software with a community
of independently operated hosts. People and organisations can sponsor compute,
connectivity or operational work. A public directory credits hosting providers and, where confirmed, sponsors,
and shows where reviewed nodes operate.

The first project node is **RIRRd UK 01**, hosted in the **United Kingdom** by
**HYEHOST LLC**, as supplied by the project owner. Its public WHOIS and HTTPS
service are checked separately. No city, financial sponsorship, service level
or community enrollment availability is inferred. Additional community nodes
and governance remain planned.
The code repository remains private until its public release is authorized.

## What a listing should tell a network operator

- Stable node identity and responsible operator, with consent to publish.
- City/region/country, rather than an invented location inferred from latency.
- Confirmed sponsor credit and an independently checked sponsor URL if used.
- WHOIS and HTTPS query endpoints, IPv4/IPv6 reachability and verification date.
- Which sources it serves, authoritative versus mirrored roles, refresh policy,
  freshness evidence and any query/filtering differences.
- Software version and compatibility baseline, status/maintenance information
  and an operator contact or published incident process.

A sponsor label in a self-hosted dashboard is operator-supplied metadata. It does
not enroll or verify a node in the community directory. Sponsorship does not grant
permission to change another source's objects or bypass maintainer authorization.

## Initial governance approach

Directory changes should be reviewable contributions with evidence and operator
consent. Operators remain accountable for licensing their imported data, source
policy, authentication, privacy, maintenance and abuse handling. The project
should publish minimum operating requirements and an unlisting/incident process
before accepting nodes. No payment integration or contractual sponsorship terms
are implemented at this checkpoint.

The dashboard consumes a small versioned `network.json` snapshot. Only confirmed
`status: verified` entries are rendered as listings; there are no sample sponsors
or fabricated nodes in the shipped directory. A future verifier must not blindly
fetch arbitrary submitted URLs: endpoint ownership, address policy, TLS and
bounded checks need review before automated enrollment.

Canonical node metadata is in `tools/live-service/node.json`; the dashboard
uses `web/dashboard/network.json`. Example empty directory shape for a new fork:

```json
{"schema_version": 1, "status": "planned", "nodes": []}
```

Future verified entries can carry `id`, `name`, `region`, `city`, `country`,
`operator`, `sponsor`, `whois`, `sources`, `verified_at` and `status`. Endpoint
availability and data freshness need timestamps, not permanent green badges.
