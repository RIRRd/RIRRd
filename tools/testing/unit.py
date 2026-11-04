#!/usr/bin/env python3
"""Export existing service and reporting suites with individual case results."""
import argparse
from pathlib import Path
import sys
import unittest
from harness import ROOT
from results import Result, write

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--suite',choices=['gateway','community','reports'],required=True)
parser.add_argument('--report',type=Path,required=True)
args=parser.parse_args()
folder={'gateway':ROOT/'tools/live-service','community':ROOT/'site/community','reports':Path(__file__).resolve().parent}[args.suite]
sys.path.insert(0,str(folder))
suite=unittest.defaultTestLoader.discover(str(folder),pattern='test_*.py')
result=unittest.TextTestRunner(verbosity=2,resultclass=Result).run(suite)
write(args.report,result.records)
raise SystemExit(not result.wasSuccessful())
