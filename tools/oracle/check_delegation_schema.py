#!/usr/bin/env python3
"""Check original AS-block/domain fixtures against the pinned official schemas."""
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import RPSLAsBlock, RPSLDomain

config_init('/etc/irrd.yaml')
contacts='admin-c: CT1-TEST\ntech-c: CT1-TEST\nmnt-by: TEST-MNT\nsource: TEST\n'
block='as-block: AS64496 - AS64511\n'+contacts
domain='domain: 2.0.192.in-addr.arpa\nzone-c: ZONE-TEST\n'+contacts
cases=[(RPSLAsBlock,block,True),(RPSLAsBlock,block.replace('AS64496 - AS64511','AS64511 - AS64496'),False),
       (RPSLAsBlock,block.replace('AS64496 - AS64511','AS0 - AS4294967295'),True),
       (RPSLDomain,domain,True),(RPSLDomain,domain.replace('zone-c: ZONE-TEST\n',''),False),
       (RPSLDomain,domain.replace('zone-c: ZONE-TEST','zone-c: ZONE-TEST, CT1-TEST'),False),
       (RPSLDomain,domain+'nserver: opaque DNS description\nsub-dom: child\ndom-net: legacy text\nrefer: referral\n',True),
       (RPSLDomain,domain+'refer: one\nrefer: two\n',False)]
for cls,text in [(RPSLAsBlock,block),(RPSLDomain,domain)]:
    for missing in ['admin-c: CT1-TEST\n','tech-c: CT1-TEST\n']:
        cases.append((cls,text.replace(missing,''),False))
    cases.append((cls,text+'auth: DUMMY\n',False))
    cases.append((cls,text.replace('admin-c: CT1-TEST','admin-c: CT1-TEST, CT2-TEST'),False))
for number,(cls,text,expected) in enumerate(cases,1):
    assert (not cls(text).messages.errors())==expected,(number,cls.__name__)
print(f'{len(cases)} AS-block/domain schema cases match the pinned baseline; write authentication tested separately')
