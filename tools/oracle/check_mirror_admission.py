#!/usr/bin/env python3
"""Original unsafe-member examples; no registry database reads or writes."""
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import RPSLAsSet
config_init('/etc/irrd.yaml')
for member in ['AS-EXA\x1fMPLE','AS-EXA\x00MPLE','AS-EXA\x7fMPLE']:
    text='as-set: AS-UNSAFE\nmembers: '+member+'\nmnt-by: TEST-MNT\nsource: TEST\n'
    assert RPSLAsSet(text,strict_validation=False).messages.errors()
print('Three unsafe member records are rejected by pinned non-strict mirror parsing')
