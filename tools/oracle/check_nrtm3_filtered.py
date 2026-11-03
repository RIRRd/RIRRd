#!/usr/bin/env python3
"""Original NRTMv3 omission/framing cases against the pinned official parser."""
from unittest.mock import Mock
from irrd.conf import config_init
from irrd.mirroring.parsers import NRTMStreamParser

config_init('/etc/irrd.yaml')
stream = ('%START Version: 3 TEST 10-15\n\nADD 11\n\naut-num: AS64496\nsource: TEST\n\n'
          'DEL 13\n\naut-num: AS64497\nsource: TEST\n\n%END TEST\n')
handler = Mock()
parsed = NRTMStreamParser('TEST', stream, handler)
assert [operation.serial for operation in parsed.operations] == [11, 13]
handler.record_serial_newest_mirror.assert_called_with('TEST', 15)
empty = NRTMStreamParser('TEST', '%START Version: 3 TEST 10-15\n\n%END TEST\n', Mock())
assert empty.operations == []
for invalid in [stream.replace('DEL 13', 'DEL 11'), stream.replace('%END TEST', '%END OTHER'), stream.replace('%END TEST', '')]:
    try:
        NRTMStreamParser('TEST', invalid, Mock())
    except ValueError:
        pass
    else:
        raise AssertionError('Invalid framing or duplicate serial was accepted')
print('Five NRTMv3 omission/framing cases match the pinned official parser; malformed objects are handled more strictly by RIRRd')
