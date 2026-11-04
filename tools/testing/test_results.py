"""Make reporting fail visibly for fixture, test, subtest and expectation errors."""
import io
import json
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET
from results import Result, write


class ExportChecks(unittest.TestCase):
    def run_case(self, cls):
        return unittest.TextTestRunner(stream=io.StringIO(), resultclass=Result).run(unittest.defaultTestLoader.loadTestsFromTestCase(cls))

    def test_setup_error_is_exported_even_when_no_test_started(self):
        class Fixture(unittest.TestCase):
            @classmethod
            def setUpClass(cls):raise RuntimeError('setup failed')
            def test_never_started(self):pass
        result=self.run_case(Fixture)
        self.assertFalse(result.wasSuccessful())
        self.assertEqual([r['status'] for r in result.records],['failed'])

    def test_failed_subtest_is_not_reported_as_a_success(self):
        class Fixture(unittest.TestCase):
            def test_failure(self):
                with self.subTest(query='synthetic'):self.fail('subtest failed')
        result=self.run_case(Fixture)
        self.assertFalse(result.wasSuccessful())
        self.assertEqual([r['status'] for r in result.records],['failed'])

    def test_expected_failure_and_skip_never_count_as_passes(self):
        class Fixture(unittest.TestCase):
            @unittest.expectedFailure
            def test_expected(self):self.fail()
            @unittest.skip('not available')
            def test_skipped(self):pass
        result=self.run_case(Fixture)
        self.assertTrue(result.wasSuccessful())
        self.assertEqual([r['status'] for r in result.records],['skipped','skipped'])

    def test_unexpected_success_is_a_failure(self):
        class Fixture(unittest.TestCase):
            @unittest.expectedFailure
            def test_unexpected(self):pass
        result=self.run_case(Fixture)
        self.assertFalse(result.wasSuccessful())
        self.assertEqual(result.records[0]['status'],'failed')

    def test_junit_is_parseable_and_public_json_omits_failure_details(self):
        with tempfile.TemporaryDirectory() as root:
            file=Path(root)/'report'
            write(file,[{'name':'case<&>', 'status':'failed', 'seconds':.1, 'detail':'private failure\x1b[31m'}])
            data=json.loads(file.with_suffix('.json').read_text())
            self.assertNotIn('detail',data[0])
            xml=ET.parse(file.with_suffix('.xml')).getroot()
            self.assertEqual(xml.attrib['failures'],'1')
            self.assertEqual(xml.find('testcase').attrib['name'],'case<&>')


if __name__=='__main__':unittest.main()
