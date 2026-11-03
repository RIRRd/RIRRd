#!/usr/bin/env python3
"""Pinned schema decisions mirrored by policy_set_tests in the Rust validator."""
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import OBJECT_CLASS_MAPPING

config_init('/etc/irrd.yaml')
common = 'admin-c: TC1-TEST\nmnt-by: TEST-MNT\nsource: TEST\n'
checks = 0

def check(cls, text, valid):
    global checks
    obj = OBJECT_CLASS_MAPPING[cls](text)
    assert (not obj.messages.errors()) == valid, f'{cls}: schema case {checks+1} differs'
    checks += 1

for cls, name, payload in [
    ('filter-set', 'FLTR-TEST', 'filter: AS64496\nmp-filter: AFI IPV6 ANY\n'),
    ('peering-set', 'PRNG-TEST', 'peering: AS64496\npeering: AS64497\nmp-peering: AS64496\n'),
    ('rtr-set', 'RTRS-TEST', 'members: router.example, AS64496:RTRS-CHILD\nmp-members: 192.0.2.1\nmbrs-by-ref: MISSING-MNT\n'),
]:
    text = f'{cls}: AS64496:{name}\n{payload}{common}'
    check(cls, text, True)
    check(cls, text.replace(name, 'WRONG-NAME'), False)
    check(cls, text+'auth: BCRYPT-PW secret\n', False)
for payload in ['', 'filter: ANY\nfilter: ANY\n', 'filter: ANY\nmp-filter: ANY\nmp-filter: ANY\n']:
    check('filter-set', f'filter-set: AS64496:FLTR-TEST\n{payload}{common}', False)
for member in ['2001:db8::1', 'router..example', 'router.example/32', 'AS64496:RS-WRONG']:
    check('rtr-set', f'rtr-set: AS64496:RTRS-TEST\nmembers: {member}\n{common}', False)
check('rtr-set', f'rtr-set: AS64496:RTRS-TEST\nmembers: router.example,,router2.example,\n{common}', True)
router = 'inet-rtr: router.example\nlocal-as: AS64496\nalias: alias.example\nmember-of: AS64496:RTRS-MISSING\n' + common
check('inet-rtr', router, True)
for bad in [router.replace('local-as: AS64496\n',''), router.replace('alias.example','bad..example'), router.replace('AS64496:RTRS-MISSING','AS64496:RS-WRONG'), router+'local-as: AS64497\n', router+'rs-in: ANY\nrs-in: ANY\n', router.replace('AS64496\n','not-an-asn\n')]:
    check('inet-rtr',bad,False)
check('inet-rtr',router+'ifaddr: 192.0.2.1 masklen 24\nifaddr: 192.0.2.2 masklen 24\npeer: AS64496\nmp-peer: AS64497\n',True)
print(f'{checks} policy-set and router schema cases match pinned IRRd')
