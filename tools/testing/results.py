"""Small standard-library JSON/JUnit exporter; skipped tests never count as passed."""
import json
import re
from pathlib import Path
import time
import unittest
import xml.etree.ElementTree as ET


class Result(unittest.TextTestResult):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.records = []
        self.started = time.monotonic()

    def startTest(self, test):
        super().startTest(test)
        self.started = time.monotonic()

    def record(self, test, status, detail=''):
        self.records.append({'name':test.id(), 'status':status, 'seconds':time.monotonic()-self.started, 'detail':detail})

    def addSuccess(self, test):
        super().addSuccess(test)
        self.record(test, 'passed')

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self.record(test, 'failed', self._exc_info_to_string(err, test))

    def addError(self, test, err):
        super().addError(test, err)
        self.record(test, 'failed', self._exc_info_to_string(err, test))

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            self.record(subtest, 'failed', self._exc_info_to_string(err, test))

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        self.record(test, 'skipped', 'Expected failure: ' + self._exc_info_to_string(err, test))

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        self.record(test, 'failed', 'Unexpected success: remove or investigate expected-failure marker')

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self.record(test, 'skipped', reason)


def write(out, records):
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    # Public JSON contains case identities and outcomes, never traces/queries.
    out.with_suffix('.json').write_text(json.dumps([{k:v for k,v in r.items() if k!='detail'} for r in records], indent=2)+'\n')
    suite = ET.Element('testsuite', name=out.name, tests=str(len(records)), failures=str(sum(r['status']=='failed' for r in records)), skipped=str(sum(r['status']=='skipped' for r in records)))
    for r in records:
        case = ET.SubElement(suite, 'testcase', name=r['name'], time=f"{r['seconds']:.6f}")
        if r['status'] == 'failed':
            ET.SubElement(case, 'failure').text = re.sub(r'[\x00-\x08\x0b\x0c\x0e-\x1f]', '', r.get('detail', ''))
        elif r['status'] == 'skipped':
            ET.SubElement(case, 'skipped').text = re.sub(r'[\x00-\x08\x0b\x0c\x0e-\x1f]', '', r.get('detail', ''))
    ET.ElementTree(suite).write(out.with_suffix('.xml'), encoding='utf-8', xml_declaration=True)
