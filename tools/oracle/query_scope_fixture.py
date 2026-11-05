#!/usr/bin/env python3
"""Synthetic two-source mirror/query fixture; no real registry or credential data."""
import argparse

SOURCES = ('QUERY-FIRST', 'QUERY-SECOND')


def records(source):
    if source not in SOURCES:
        raise ValueError('Select one of the dedicated query oracle sources')
    first = source == SOURCES[0]
    child = 'AS64497' if first else 'AS64496'
    member = 'AS64496' if first else 'AS64498'
    origin = 'AS64497' if first else 'AS64498'
    contact = 'QUERY1-TEST'
    mnt = 'QUERY-MNT'
    common = f'mnt-by: {mnt}\nsource: {source}\n'
    base = [
        'as-block: AS64496 - AS64511\n',
        'as-set: AS-DUPLICATE\nmembers: '+member+', AS-CHILD, AS-CYCLE\n',
        'as-set: AS-CHILD\nmembers: '+child+'\n',
        'as-set: AS-CYCLE\nmembers: AS-DUPLICATE\n',
        'as-set: AS-INDIRECT\nmbrs-by-ref: QUERY-MNT\n',
        'as-set: AS-EMPTY\n',
        f'aut-num: AS64496\nas-name: {source}\nmember-of: AS-INDIRECT\n',
        f'aut-num: {origin}\nas-name: QUERY-CHILD\n',
        'domain: 2.0.192.in-addr.arpa\n',
        'filter-set: FLTR-QUERY\nfilter: AS64496\n',
        'inet-rtr: r1.example.invalid\nlocal-as: AS64496\n',
        'inet6num: 2001:db8::/32\n',
        'inetnum: 192.0.2.0 - 192.0.2.255\n',
        'irt: IRT-QUERY\n',
        'key-cert: PGPKEY-A1B2C3D4\n',
        f'mntner: {mnt}\nauth: BCRYPT-PW DummyValue\n',
        'organisation: ORG-QUERY\norg-name: Synthetic query fixture\n',
        'peering-set: PRNG-QUERY\npeering: AS64496\n',
        f'person: Query Fixture\nnic-hdl: {contact}\n',
        'role: Query Fixture NOC\nnic-hdl: QNOC1-TEST\n',
        'route: 192.0.2.0/24\norigin: AS64496\n',
        ('route: 198.51.100.0/24\n' if first else 'route: 203.0.113.0/24\n')+'origin: '+origin+'\n',
        'route-set: RS-QUERY\nmembers: AS-DUPLICATE, 192.0.2.0/24\nmp-members: 2001:db8:ffff::/48\n',
        ('route6: 2001:db8:1::/48\n' if first else 'route6: 2001:db8:3::/48\n')+'origin: AS64496\n',
        ('route6: 2001:db8:2::/48\n' if first else 'route6: 2001:db8:4::/48\n')+'origin: '+origin+'\n',
        'rtr-set: RTRS-QUERY\nmembers: r1.example.invalid\n',
    ]
    result = [text+common for text in base]
    # Matching member-of alone must not bypass mbrs-by-ref maintainer matching.
    result.append('aut-num: AS64499\nas-name: EXCLUDED-INDIRECT\nmember-of: AS-INDIRECT\nmnt-by: OTHER-MNT\nsource: '+source+'\n')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, choices=SOURCES)
    print('\n'.join(records(parser.parse_args().source)))
