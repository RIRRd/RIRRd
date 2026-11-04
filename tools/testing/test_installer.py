"""Verify release selection and checksum failure without network or package installs."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[2]

@unittest.skipUnless(sys.platform.startswith('linux'), 'Linux release installer')
class Installer(unittest.TestCase):
    def run_installer(self,arch,format,manifest='valid'):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);bin_dir=root/'bin';bin_dir.mkdir();output=root/'out'
            archive=f'rust-irrd_0.1.0~beta.2-1_{"amd64" if arch=="x86_64" else "arm64"}.deb' if format=='deb' else f'rust-irrd-0.1.0-0.1.beta.2.{arch}.rpm'
            content=b'isolated synthetic package';digest=hashlib.sha256(content).hexdigest()
            text=f'{digest}  ./{archive}\n'
            if manifest=='corrupt':text='0'*64+f'  ./{archive}\n'
            if manifest=='duplicate':text+=text
            if manifest=='wrong':text=f'{digest}  ./unrelated.deb\n'
            (root/'package').write_bytes(content);(root/'manifest').write_text(text)
            (bin_dir/'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo '+arch+';; esac\n')
            (bin_dir/'curl').write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args=sys.argv[1:];dest=pathlib.Path(args[args.index('--output')+1]);url=args[-1]
assert url.startswith('https://github.com/RIRRd/RIRRd/releases/download/v0.1.0-beta.2/')
shutil.copyfile(pathlib.Path(os.environ['INSTALL_FIXTURE'])/('manifest' if url.endswith('/SHA256SUMS') else 'package'),dest)
''')
            for path in bin_dir.iterdir():path.chmod(0o755)
            env={**os.environ,'PATH':str(bin_dir)+os.pathsep+os.environ['PATH'],'INSTALL_FIXTURE':str(root)}
            command=['bash',str(ROOT/'tools/install.sh'),'--format',format,'--download-only',str(output)]
            result=subprocess.run(command,env=env,text=True,capture_output=True,timeout=10)
            if manifest=='valid' and arch in ['x86_64','aarch64']:
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual((output/archive).read_bytes(),content)
                self.assertEqual((output/archive).stat().st_mode&0o777,0o644)
                again=subprocess.run(command,env=env,text=True,capture_output=True,timeout=10)
                self.assertNotEqual(again.returncode,0)
            else:
                self.assertNotEqual(result.returncode,0)
                self.assertFalse(output.exists())
    def test_debian_x86(self):self.run_installer('x86_64','deb')
    def test_debian_arm(self):self.run_installer('aarch64','deb')
    def test_rpm_x86(self):self.run_installer('x86_64','rpm')
    def test_rpm_arm(self):self.run_installer('aarch64','rpm')
    def test_checksum_mismatch(self):self.run_installer('x86_64','deb','corrupt')
    def test_duplicate_manifest(self):self.run_installer('x86_64','deb','duplicate')
    def test_missing_package_in_manifest(self):self.run_installer('x86_64','deb','wrong')
    def test_unsupported_architecture(self):self.run_installer('riscv64','deb')
