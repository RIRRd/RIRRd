# Authoritative allocation and incident-response objects

The beta.4 development changes add authenticated create, modify and delete
operations for `inetnum`, `inet6num` and `irt`. Eighteen of the nineteen mirrored
RPSL classes now have an authoritative submission profile. `key-cert` remains
unsupported for authoritative updates pending certificate and OpenPGP work.
This is not complete IRRd authorization or schema parity.

The reference is official IRRd **4.5.4**, commit
`8e8080dab744a348b7d0e6efcb53f403301dd075`:
[object schemas](https://github.com/irrdnet/irrd/blob/8e8080dab744a348b7d0e6efcb53f403301dd075/irrd/rpsl/rpsl_objects.py),
[field validation](https://github.com/irrdnet/irrd/blob/8e8080dab744a348b7d0e6efcb53f403301dd075/irrd/rpsl/fields.py),
and [authorization/reference checks](https://github.com/irrdnet/irrd/blob/8e8080dab744a348b7d0e6efcb53f403301dd075/irrd/updates/validators.py).

## Implemented behavior

- Allocations require their class identity, `netname`, `country`, `admin-c`,
  `tech-c`, `status`, `mnt-by` and `source`. `country` may repeat; `netname`,
  `status` and `geofeed` may not. Country, status and network-name values use the
  pinned text-field profile, without adding a registry-specific vocabulary.
- IPv4 allocations accept single addresses and ordered ranges, including ranges
  that are not CIDR aligned. IPv6 allocation prefixes reject host bits and
  mismatched address families. Unlike authoritative routes, allocations may
  contain the zero prefix or unspecified address.
- Optional `geofeed` metadata requires an HTTP(S) URL with a network location.
  Bracketed IPv6, scoped IPv6 and IPvFuture values are checked, as are the pinned
  Python runtime's Unicode 15.1 NFKC authority delimiter characters. This field
  is **stored metadata**: the daemon does not download or resolve it.
- IRT objects require their name, `address`, `e-mail`, `abuse-mailbox`, `mnt-by`
  and `source`. Addresses and mailboxes may repeat; contacts are optional.
- Existing and proposed maintainers must authenticate within the same source.
  Contact references must resolve to a person or role in that source. References
  prevent deleting a contact still used by an allocation or IRT object.
- Newly created allocations immediately participate in the existing same-source
  route-parent authorization checks. Creating an allocation itself follows the
  pinned maintainer rules; no additional allocation-parent proof is invented.
- All entry points retain the existing atomic batch contract, journal/audit
  updates, credential redaction, bounded validation and opt-in authoritative
  source configuration. No database tables, indexes or file format changed.

`mnt-lower`, `mnt-routes` and other fields outside the pinned class schemas are
not added implicitly. Existing limits, including 32 changes per atomic batch,
64 KiB authoritative records and password-work bounds, remain in force.

## Repeatable verification

[The shared synthetic corpus](../../tests/fixtures/authoritative-allocations.json)
contains 157 acceptance/rejection cases. Both the Rust unit test and official
IRRd parser consume the same records. It covers required fields, multiplicity,
address families and ranges, reserved names, repeated references, optional
fields, email syntax, folded text and malformed geofeed URLs.

The [normal CLI/HTTP test](../../tests/authoritative_allocations.rs) creates all
three classes, checks source and contact isolation, proves failed batches leave
byte-identical exports, tests route creation against the new allocations,
transfers maintainers, verifies protected-contact deletion and whole-batch HTTP
rollback, retrieves the objects through GraphQL, deletes a complete reference
batch, and reopens/verifies the database and committed journal serial.

```sh
cargo test --locked allocation
python3 tools/oracle/check_allocation_schema.py
python3 tools/oracle/compare_allocation_auth.py --report /private/allocation-oracle.json
```

The HTTP comparator requires the existing isolated oracle AUTH-TEST fixture and
candidate described in [the oracle workflow](../../.github/workflows/upstream-oracle.yml).
It compares the same schema corpus through `/v1/submit/`, then tests both
maintainer proofs, source isolation, strong references, persisted GraphQL
results and both families of route-parent authorization. It sends no updates to
public registries. Test credentials and objects are synthetic.

Local development verification on 5 November 2026 passed all **157 schema
cases**, the normal CLI/HTTP integration test and **256 HTTP/query case groups**
against both implementations. The comparison records a unique contact suffix
so repeated runs retain, rather than clear, deleted-name protection. Supply
`--case-id` to reproduce that suffix against a clean fixture. These local
results precede final release-artifact and CI qualification.

Malformed bracketed or NFKC-delimiter geofeed URLs can raise `ValueError` inside
the pinned Python URL parser; its submission handler returns HTTP 500. RIRRd
returns a bounded rejection. Those specific reference exceptions are marked in
the fixture and checked explicitly; arbitrary upstream failures never count as
passing comparisons. Malformed RPSL can also produce RIRRd HTTP 400 where IRRd
returns a parser-error report with HTTP 200. Neither outcome commits an object.

RIRRd rejects an entire failing batch; upstream can accept independent objects
within a mixed submission. Accordingly, atomic rollback is tested separately
and is not misreported as identical upstream partial-success semantics.
The corpus establishes the listed cases, not equivalence for every email,
Unicode normalization or legacy parser edge case.
