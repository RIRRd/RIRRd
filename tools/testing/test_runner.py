"""Exercise report parsing and fail-closed execution rather than trusting green output."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from run import ROOT, rust_cases


class RunnerChecks(unittest.TestCase):
    def test_rust_results_keep_target_identity_and_ignored_status(self):
        text='''Running unittests src/main.rs (target/debug/deps/main)
test module::ok ... ok
test module::helper ... ignored, fixture helper
Running tests/smoke.rs (target/debug/deps/smoke)
test contract ... FAILED
'''
        records=rust_cases(text)
        self.assertEqual([r['status']for r in records],['passed','skipped','failed'])
        self.assertEqual(records[-1]['name'],'tests/smoke.rs::contract')

    def test_build_failure_produces_failed_report_and_junit(self):
        if os.name=='nt':self.skipTest('POSIX executable fixture; Windows still runs exporter and Rust/protocol tests')
        with tempfile.TemporaryDirectory(prefix='rirrd-runner-failure-')as root:
            root=Path(root);bin=root/'bin';bin.mkdir()
            fake=bin/'cargo';fake.write_text('#!/bin/sh\necho deliberate-build-failure\nexit 7\n');fake.chmod(0o755)
            out=root/'report';out.mkdir()
            (out/'report.json').write_text('{"status":"passed"}')
            (out/'web-cases.xml').write_text('stale success')
            result=subprocess.run([sys.executable,str(ROOT/'tools/testing/run.py'),'--profile','web','--out',str(out)],env={**os.environ,'PATH':str(bin)+os.pathsep+os.environ['PATH']},capture_output=True,timeout=15)
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((out/'web-cases.xml').exists())
            data=json.loads((out/'report.json').read_text())
            self.assertEqual(data['status'],'failed')
            self.assertEqual(data['groups'][0]['exit_code'],7)
            self.assertIn('<failure>',(out/'junit.xml').read_text())
            self.assertFalse(any(g['id']=='web' for g in data['groups']))


if __name__=='__main__':unittest.main()
