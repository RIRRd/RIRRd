#!/usr/bin/env python3
"""Generate synthetic oracle credentials; never use these test passwords in a service."""
import bcrypt

contact = ('person: Oracle Contact\naddress: Documentation Road\nphone: +1 202 555 0100\n'
           'e-mail: oracle@example.invalid\nnic-hdl: CT1-AUTH\nmnt-by: PARENT-MNT\nsource: AUTH-TEST\n')
print(contact)
for name, password in [('PARENT-MNT', b'oracle parent proof'), ('OWNER-MNT', b'oracle owner proof')]:
    hashed = bcrypt.hashpw(password, bcrypt.gensalt(rounds=4)).decode()
    print(f'mntner: {name}\nadmin-c: CT1-AUTH\nupd-to: oracle@example.invalid\nauth: BCRYPT-PW {hashed}\nmnt-by: {name}\nsource: AUTH-TEST\n')
print('aut-num: AS64496\nas-name: AS-ORACLE\nadmin-c: CT1-AUTH\ntech-c: CT1-AUTH\nmnt-by: PARENT-MNT\nsource: AUTH-TEST\n')
# Deliberately dangling contact from a trusted snapshot, with no tombstone.
# Normal authenticated updates must not claim its handle (upstream issue #616).
print('domain: dangling.example\nadmin-c: CT1-AUTH\ntech-c: CT1-AUTH\nzone-c: DANGLING-AUTH\nmnt-by: OWNER-MNT\nsource: AUTH-TEST\n')
