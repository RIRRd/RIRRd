# IRR object coverage profiles

The daemon recognises the 19 RPSL object classes supported by the pinned
IRRd 4.5.4 baseline. Snapshot structural parsing and query support do not imply
complete authoritative validation or authentication for every class.

The full profile retains:

```text
as-block as-set aut-num domain filter-set inet-rtr inet6num inetnum irt
key-cert mntner organisation peering-set person role route route-set route6 rtr-set
```

A routing-only profile can retain `aut-num`, `as-set`, `route-set`, `route`,
`route6`, `filter-set`, `peering-set`, `rtr-set` and `inet-rtr`. This reduces data
volume but omits allocation, contact and maintainer metadata. Source object totals
must identify their retained classes; compare matching classes when evaluating
another mirror. Counts are per source/class/key, not unique announced prefixes.

## Mirroring and initial serials

Never attach a current feed serial to an older snapshot. Verify the publisher's
snapshot serial before and after downloading, import the matching snapshot, then
catch up from that serial. A secondary mirror can use a different serial namespace.
The configured source class filter must match the imported object scope.

For large initial backlogs, the CLI accepts an inclusive `--end-serial`:

```sh
rust-irrd --config irrd.toml source sync-nrtm3 --source EXAMPLE \
  --endpoint 192.0.2.10:43 --base-serial 100 --end-serial 1100
# Later batches use the stored cursor and omit --base-serial.
```

The peer must serve that range. Responses exceeding the requested end are
rejected before applying objects. Network staging remains bounded to 64 MiB;
large datasets do not require an unbounded update response in memory or on disk.
A complete recognised no-update response may have a publisher comment banner.
Server errors cannot be hidden by a later no-update warning.

## Public rendering and receipts

Maintainer `auth:` attributes are retained privately for data fidelity and
removed from public WHOIS, HTTP, GraphQL and journal output. Public key-cert
`certif:` material is public-key metadata. Snapshot submission credentials remain
invalid. Raw CLI snapshot exports require `--include-sensitive`; use `--public`
for a redacted export with its source serial.

A source inventory should retain import times, current per-class counts,
excluded class counts, malformed-record counts, transport details and file
checksums. A full standard-IRRd profile can still encounter publisher-specific
classes such as RIPE poems; record those unsupported classes explicitly.
NRTMv3 transport is unauthenticated. Full object coverage and a green test run
are not a production-readiness claim.

Published snapshot objects may be up to 8 MiB; the full RIPE export includes an
inet6num record slightly above 4 MiB. Authoritative submissions retain their
64 KiB object limit. Query and response limits remain bounded independently; an
over-limit answer fails instead of silently truncating the stored object.

Public rendering also withholds private-key armor from certificate data, including
folded payload lines. Explanatory header mentions in remarks remain visible.
This is a disclosure guard; cryptographic OpenPGP/key-cert authentication
workflows remain part of the authoritative parity backlog.

A mirror may set `encoding = "iso-8859-1"` in its `[nrtm3_sync.SOURCE]`
section. The default is strict `utf-8`. The CLI uses the configured encoding
only when its endpoint matches that source's configured peer; the scheduler
uses the same reader. Wire bytes and decoded object sizes have separate bounds.
Malformed default UTF-8 rolls back the complete update and its cursor.
RIPE documents Latin-1 for its NRTMv3 interface even when the selected baseline
is a UTF-8 export: [interface encoding](https://docs.db.ripe.net/RIPE-Database-Structure/Character-Encoding/).
Known idle replies are matched exactly, including the older PANIX wording;
arbitrary warnings or appended error/data lines do not count as successful polls.

Source providers can publish derived conversion views alongside primary IRRs.
The console displays supplied source provenance and refresh notes. Public object
rendering removes Registro.BR owner-identifier remarks and their continuations;
prefixes, origins and conversion provenance remain available. Raw, explicitly
restricted operator exports retain the original records.
