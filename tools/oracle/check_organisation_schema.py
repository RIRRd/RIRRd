#!/usr/bin/env python3
"""Run inside the pinned oracle image; only synthetic objects, no database writes."""
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import RPSLOrganisation

config_init('/etc/irrd.yaml')
text = (
    'organisation: ORG-TEST\norg-name: Example Organisation\ncountry: GB\n'
    'address: Example Road\ne-mail: noc@example.net\nmnt-by: TEST-MNT\nsource: TEST\n'
)
assert not RPSLOrganisation(text).messages.errors()
for missing in ('org-name: Example Organisation\n', 'country: GB\n',
                'address: Example Road\n', 'e-mail: noc@example.net\n'):
    assert RPSLOrganisation(text.replace(missing, '')).messages.errors()
for extra in ('org-name: Duplicate\n', 'auth: secret\n', 'e-mail: invalid\n',
              'admin-c: TC1-TEST, TC2-TEST\n'):
    assert RPSLOrganisation(text + extra).messages.errors()
assert not RPSLOrganisation(text + 'country: NL\naddress: Second line\ne-mail: second@example.net\n').messages.errors()
print('10 pinned organisation schema cases passed')
