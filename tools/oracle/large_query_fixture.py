#!/usr/bin/env python3
"""Generate deterministic private-address data for large IRR query comparisons."""
import ipaddress
import sys

SOURCE = 'LARGE'
CHILDREN = 1300
ASNS_PER_CHILD = 4
ORIGINS = CHILDREN * ASNS_PER_CHILD
FIRST_ASN = 4200001000
SINGLE_ASN = 4200000000
INDIRECT_MEMBERS = 257


def child_name(index):
    return f'AS-LARGE-CHILD-{index:04d}'


def prefix(index, version, single=False):
    if version == 4:
        base = ipaddress.IPv4Address('10.128.0.0' if single else '10.0.0.0')
        return f'{base + index}/32'
    base = ipaddress.IPv6Address('fd13:3456:789a:bcde::' if single else 'fd12:3456:789a:bcde::')
    return f'{base + index}/128'


def records():
    common = f'mnt-by: LARGE-MNT\nsource: {SOURCE}\n\n'
    yield 'mntner: LARGE-MNT\nauth: BCRYPT-PW DummyValue\n'+common
    yield 'as-set: AS-LARGE\nmembers: '+', '.join(child_name(i) for i in range(CHILDREN))+'\n'+common
    yield f'as-set: AS-LARGE-ORIGIN\nmembers: AS{SINGLE_ASN}\n'+common
    yield 'as-set: AS-LARGE-INDIRECT\nmbrs-by-ref: LARGE-MNT\n'+common
    for index in range(CHILDREN):
        members = [f'AS{FIRST_ASN + index * ASNS_PER_CHILD + offset}' for offset in range(ASNS_PER_CHILD)]
        if index == 0:
            members.append('AS-LARGE')  # A cycle must not duplicate the answer.
        yield f'as-set: {child_name(index)}\nmembers: '+', '.join(members)+'\n'+common
    for index in range(ORIGINS):
        asn = FIRST_ASN + index
        if index < INDIRECT_MEMBERS:
            yield f'aut-num: AS{asn}\nas-name: LARGE-MEMBER\nmember-of: AS-LARGE-INDIRECT\n'+common
        for offset in range(2):
            yield f'route: {prefix(index * 2 + offset, 4)}\norigin: AS{asn}\n'+common
        yield f'route6: {prefix(index, 6)}\norigin: AS{asn}\n'+common
        for version, object_class in [(4, 'route'), (6, 'route6')]:
            yield f'{object_class}: {prefix(index, version, single=True)}\norigin: AS{SINGLE_ASN}\n'+common
    # A member-of relation with the wrong maintainer must remain excluded even
    # when the implementation switches to indexed large membership traversal.
    yield f'aut-num: AS4200999999\nas-name: WRONG-MAINTAINER\nmember-of: AS-LARGE-INDIRECT\nmnt-by: OTHER-MNT\nsource: {SOURCE}\n\n'


if __name__ == '__main__':
    for record in records():
        sys.stdout.write(record)
