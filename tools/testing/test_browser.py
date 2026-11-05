"""A failed browser startup must terminate its process and preserve diagnostics."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from check_dashboard import Browser

@unittest.skipUnless(sys.platform.startswith('linux'),'POSIX browser process groups')
class BrowserStartup(unittest.TestCase):
    def check_failure(self,malformed):
        with tempfile.TemporaryDirectory()as temp:
            root=Path(temp);pid_file=root/'pid';executable=root/'fixture'
            executable.write_text('#!/usr/bin/env python3\nimport os,time\nfrom pathlib import Path\nPath('+repr(str(pid_file))+').write_text(str(os.getpid()))\nos.write(2,b"synthetic startup diagnostic\\n")\n'+('os.write(4,b"malformed\\0")\n' if malformed else '')+'while True:time.sleep(1)\n')
            executable.chmod(0o755)
            with self.assertRaises(RuntimeError)as raised:
                Browser(str(executable),str(root/'profile'),startup_timeout=1)
            self.assertIn('synthetic startup diagnostic',str(raised.exception))
            if not malformed:self.assertIn('browser timed out: Target.createTarget',str(raised.exception))
            pid=int(pid_file.read_text())
            with self.assertRaises(ProcessLookupError):os.kill(pid,0)
    def test_timeout_cleans_up_browser(self):self.check_failure(False)
    def test_bad_devtools_output_cleans_up_browser(self):self.check_failure(True)
