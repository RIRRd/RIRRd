#!/usr/bin/env python3
"""Pinned parser check using synthetic contacts; no database writes."""
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import rpsl_object_from_text

config_init('/etc/irrd.yaml')
cases = 0
for kind in ['person', 'role']:
    for strict in [False, True]:
        text = (kind + ':\naddress: Example Road\nphone: +1 202 555 0100\n'
                'e-mail: noc@example.invalid\nnic-hdl: EX1-TEST\n'
                'mnt-by: TEST-MNT\nsource: TEST\n')
        contact = rpsl_object_from_text(text, strict_validation=strict)
        assert not contact.messages.errors(), contact.messages.errors()
        assert contact.pk() == 'EX1-TEST'
        assert rpsl_object_from_text(text.replace('nic-hdl: EX1-TEST\n', ''),
                                     strict_validation=strict).messages.errors()
        cases += 2
print(f'{cases} pinned empty-contact-name cases passed')
