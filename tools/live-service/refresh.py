#!/usr/bin/env python3
"""Root-owned generation switch: validate while serving; rollback on failed start."""
import datetime
import argparse
import fcntl
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import time
import urllib.request

ROOT = Path('/var/lib/rirrd-live')
TOOLS = Path('/usr/local/lib/rirrd-live')
BINARY = '/usr/local/libexec/rirrd-live'


def save_state(value):
    path = ROOT / '.refresh-state'
    path.write_text(json.dumps(value))
    path.chmod(0o644)
    path.replace(ROOT / 'last-refresh.json')


def switch(target):
    temporary = ROOT / '.next'
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target)
    temporary.replace(ROOT / 'current')


def healthy(sources):
    body = json.dumps({'query': '{ databaseStatus { source } }'}).encode()
    for _ in range(20):
        try:
            request = urllib.request.Request('http://127.0.0.1:9280/graphql/', body,
                                            {'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=2) as response:
                actual = {row['source'] for row in json.load(response)['data']['databaseStatus']}
                if actual == sources:
                    return
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(1)
    raise RuntimeError('Candidate service did not pass the normal HTTP status check')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resume', help='Private failed generation name, never the serving generation')
    args = parser.parse_args()
    with (ROOT / 'refresh.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        destination = ROOT / 'generations' / datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        previous = (ROOT / 'current').resolve(strict=True)
        if args.resume:
            if not re.fullmatch(r'[A-Za-z0-9_-]+', args.resume):
                raise ValueError('Resume requires a generation basename')
            destination = ROOT / 'generations' / args.resume
            if destination.is_symlink() or destination.resolve() == previous:
                raise ValueError('The serving generation cannot be resumed or modified')
        try:
            save_state({'state': 'running', 'generation': destination.name, 'attempted_at': datetime.datetime.now(datetime.timezone.utc).isoformat()})
            subprocess.run([sys.executable, str(TOOLS / 'prepare.py'), '--catalog', str(TOOLS / 'sources.json'),
                            '--binary', BINARY, '--output', str(destination)] + (['--resume'] if args.resume else []), check=True)
            report = json.loads((destination / 'sources.json').read_text())
            before = json.loads((previous / 'sources.json').read_text())
            baseline = {row['source']: row['objects'] for row in before['sources'] if row['state'] == 'loaded'}
            loaded = {row['source']: row['objects'] for row in report['sources'] if row['state'] == 'loaded'}
            for source, count in baseline.items():
                if loaded.get(source, 0) < count * .8:
                    raise ValueError('Unexpected source loss/shrink; explicit operator review required')
            import pwd
            user = pwd.getpwnam('rust-irrd')
            destination.chmod(0o755)
            (destination / 'irrd.toml').chmod(0o644)
            (destination / 'sources.json').chmod(0o644)
            os.chown(destination / 'registry.redb', user.pw_uid, user.pw_gid)
            (destination / 'registry.redb').chmod(0o600)
            switch(destination)
            try:
                subprocess.run(['systemctl', 'restart', 'rirrd-live.service'], check=True)
                healthy({row['source'] for row in report['sources']
                         if row['state'] == 'loaded' and row.get('http_access', True)})
            except Exception:
                switch(previous)
                subprocess.run(['systemctl', 'restart', 'rirrd-live.service'], check=True)
                raise
            shutil.rmtree(destination / 'snapshots')
            # Retain the serving and immediately previous complete generations.
            for old in (ROOT / 'generations').iterdir():
                if old not in (destination, previous) and old.is_dir():
                    shutil.rmtree(old)
            save_state({'state': 'ok', 'completed_at': report['verified_at']})
        except Exception:
            # Leave serving state unchanged; private journal has detailed diagnostics.
            save_state({'state': 'failed', 'attempted_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'generation': destination.name,
                'message': 'Refresh failed; last verified generation retained. Operator review required.'})
            raise


if __name__ == '__main__':
    main()
