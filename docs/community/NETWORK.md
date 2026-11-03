# Community IRR network — first node

The intended service combines MIT-licensed registry software with a community
of independently operated hosts. People and organisations can sponsor compute,
connectivity or operational work. A public directory credits hosting providers and, where confirmed, sponsors,
and shows where reviewed nodes operate.

The first project node is **RIRRd UK 01**, hosted in the **United Kingdom** by
**HYEHOST LLC**, as supplied by the project owner. Its public WHOIS and HTTPS
service are checked separately. No city, financial sponsorship, service level
is inferred. The public website accepts private, reviewed node submissions;
additional verified nodes and broader governance remain to be established.
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

The community website consumes the reviewed `site/community/nodes.json` directory.
Its node-submission service stores operator contacts privately and returns a pending
receipt. It never fetches submitted endpoints or publishes an unreviewed listing.
The private review CLI exports only public node fields; changes are then reviewed
in Git and deployed as static assets. See the [service and review workflow](../../site/community/README.md).

The self-hosted console does not fetch the community directory, ship its listings,
or require a community account. `web/dashboard/network.json` is an empty compatibility
response for independent instances. Live public-node metadata comes separately from
`tools/live-service/node.json`; a hosting provider is not inferred to be a financial
sponsor.

Directory entries carry identity, location, hosting credits, query endpoints and
verification provenance. Endpoint availability and source freshness are live node
measurements, rather than permanent green directory badges.
