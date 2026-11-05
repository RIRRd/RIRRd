#!/usr/bin/env python3
"""Check the shared synthetic allocation/IRT corpus in pinned official IRRd."""
import argparse
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--project', default='irrd-454-oracle')
parser.add_argument('--fixture', type=Path, default=Path('tests/fixtures/authoritative-allocations.json'))
args = parser.parse_args()
# The reference image is pinned by compose.yaml; only its parser runs here.
program = '''
import json,sys
from irrd.conf import config_init
from irrd.rpsl.rpsl_objects import rpsl_object_from_text
config_init('/etc/irrd.yaml')
cases=json.load(sys.stdin)
for case in cases:
    try:
        result=rpsl_object_from_text(case['text'])
        accepted=not result.messages.errors()
        errors=result.messages.errors()
    except (ValueError, KeyError) as error:
        accepted=False
        errors=[str(error)]
    assert accepted == case['valid'], (case['name'],case['valid'],errors)
print(f'{len(cases)} shared allocation/IRT schema cases matched pinned IRRd 4.5.4')
'''
subprocess.run(['docker', 'compose', '-p', args.project, '-f', 'tools/oracle/compose.yaml',
                'exec', '-T', 'reference', 'python', '-c', program],
               input=args.fixture.read_bytes(), check=True, timeout=30)
