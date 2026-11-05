#!/usr/bin/env python3
"""Synthetic uppercase display names isolate shared indexed-filter behavior."""
print('''person: LOOKUP PERSON
nic-hdl: LOOKUP1-TEST
source: QUERY-LOOKUP

role: LOOKUP ROLE
nic-hdl: LOOKUP2-TEST
source: QUERY-LOOKUP
''')
