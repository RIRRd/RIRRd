"""Missing source data and a missed coverage floor must fail the build."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from harness import ROOT


class CoverageChecks(unittest.TestCase):
    def test_missing_project_sources_cannot_produce_a_green_report(self):
        with tempfile.TemporaryDirectory()as root:
            root=Path(root);raw=root/'raw.json';raw.write_text('{"data":[{"files":[]}]}')
            result=subprocess.run([sys.executable,str(ROOT/'tools/testing/coverage_summary.py'),str(raw),'--out',str(root/'summary.json')],capture_output=True,timeout=10)
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((root/'summary.json').exists())

    def test_missed_floor_records_the_real_percentage_and_fails(self):
        with tempfile.TemporaryDirectory()as root:
            root=Path(root);raw=root/'raw.json';raw.write_text(json.dumps({'data':[{'files':[{'filename':str(ROOT/'src/main.rs'),'summary':{'lines':{'covered':1,'count':100,'percent':1}}}]}]}))
            result=subprocess.run([sys.executable,str(ROOT/'tools/testing/coverage_summary.py'),str(raw),'--out',str(root/'summary.json'),'--minimum','92'],capture_output=True,timeout=10)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(json.loads((root/'summary.json').read_text())['lines']['percent'],1)
