#!/usr/bin/env python3
"""Check that published asset names and server-side digests match SHA256SUMS."""
import argparse
import json
from pathlib import Path
import re

def verify(manifest,release):
    expected={}
    for line in manifest.splitlines():
        match=re.fullmatch(r'([a-f0-9]{64})  (?:\./)?([A-Za-z0-9_.-]+)',line)
        if not match or match[2]in expected:raise ValueError('Unsafe or duplicate checksum manifest entry')
        expected[match[2]]='sha256:'+match[1]
    actual={asset['name']:asset.get('digest') for asset in release['assets'] if asset['name']!='SHA256SUMS'}
    if not expected or expected!=actual:raise ValueError('Published filenames/digests differ from the checksum manifest')
    return len(expected)

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('manifest',type=Path);parser.add_argument('release',type=Path);args=parser.parse_args()
    count=verify(args.manifest.read_text(),json.loads(args.release.read_text()));print(f'Verified {count} published asset names and SHA-256 digests')
if __name__=='__main__':main()
