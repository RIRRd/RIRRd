#!/usr/bin/env python3
"""Measure bounded rejection of large synthetic inverse results, without a live registry."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--before', type=Path, required=True)
parser.add_argument('--candidate', type=Path, required=True)
parser.add_argument('--before-commit', required=True)
parser.add_argument('--candidate-commit', required=True)
parser.add_argument('--out', type=Path, required=True)
parser.add_argument('--cpu', type=int, default=0)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
out = args.out.resolve()
assert not out.is_relative_to(root), 'Keep fixtures and reports outside Git'
assert args.cpu in os.sched_getaffinity(0)
out.mkdir(mode=0o700, parents=True, exist_ok=True)
run = Path(tempfile.mkdtemp(prefix='query-budget-', dir=out))
fixture = run / 'fixture.rpsl'
with fixture.open('w') as stream:
    for n in range(17):
        stream.write(f'filter-set: FLTR-STRESS-{n:02}\nfilter: ' + 'AS64496 OR ' * 150000
                     + 'AS64497\nmnt-by: MNT-STRESS\nsource: STRESS\n\n')
config = run / 'registry.toml'
config.write_text('database="registry.redb"\nsources=["STRESS"]\nlisten="127.0.0.1:0"\n'
                  'database_cache_mib=8\nworker_threads=1\nmax_blocking_threads=4\n'
                  'max_connections=4\nio_timeout_seconds=10\nmax_snapshot_bytes=67108864\n')
binaries = {'before': args.before.resolve(), 'candidate': args.candidate.resolve()}


def cli(binary, *arguments):
    return subprocess.run([str(binary), '--config', str(config), *arguments], check=True,
                          capture_output=True, timeout=120).stdout


def query(address, text):
    with socket.create_connection(address, timeout=10) as client:
        client.sendall(text)
        result = bytearray()
        while block := client.recv(65536):
            result.extend(block)
            assert len(result) <= 1024
        return bytes(result)


assert json.loads(cli(binaries['before'], 'import', 'snapshot', '--source', 'STRESS', '--file', str(fixture)))['objects'] == 17
assert json.loads(cli(binaries['before'], 'database', 'verify'))['objects'] == 17
results = {'scope': '17 synthetic large filter-set records; 32 rejected inverse queries per fresh process. Process RSS, not cgroup memory. Warm OS file cache is permitted. No public traffic.',
           'before_commit': args.before_commit, 'candidate_commit': args.candidate_commit,
           'fixture_bytes': fixture.stat().st_size, 'fixture_sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
           'binary_sha256': {name: hashlib.sha256(binary.read_bytes()).hexdigest() for name, binary in binaries.items()},
           'server_cpu': args.cpu, 'cache_mib': 8, 'runs': []}
for repetition in range(3):
    for name in (['before', 'candidate'] if repetition % 2 == 0 else ['candidate', 'before']):
        log = (run / f'{name}-{repetition}.log').open('w+')
        process = subprocess.Popen(['taskset', '-c', str(args.cpu), str(binaries[name]), '--config', str(config), 'server'],
                                   stdout=subprocess.DEVNULL, stderr=log)
        try:
            deadline = time.monotonic() + 15
            address = None
            while time.monotonic() < deadline:
                assert process.poll() is None, 'server exited'
                log.seek(0)
                for line in log:
                    fields = json.loads(line).get('fields', {})
                    if fields.get('message') == 'WHOIS listening':
                        host, port = fields['address'].rsplit(':', 1)
                        address = (host, int(port))
                if address:
                    break
                time.sleep(.05)
            assert address
            times = []
            for _ in range(32):
                start = time.perf_counter()
                assert query(address, b'!oMNT-STRESS\n') == b'F Too many results\n'
                times.append((time.perf_counter() - start) * 1000)
            assert b'rust-irrd' in query(address, b'!v\n')
            status = dict(line.split(':', 1) for line in Path(f'/proc/{process.pid}/status').read_text().splitlines() if ':' in line)
            times.sort()
            results['runs'].append({'variant': name, 'repetition': repetition + 1,
                                    'p50_ms': statistics.median(times), 'p95_ms': times[30], 'max_ms': max(times),
                                    'rss_bytes': int(status['VmRSS'].split()[0]) * 1024,
                                    'peak_rss_bytes': int(status['VmHWM'].split()[0]) * 1024,
                                    'validated_rejections': len(times)})
        finally:
            process.terminate()
            process.wait(timeout=15)
            log.close()
        (run / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
assert json.loads(cli(binaries['candidate'], 'database', 'verify'))['objects'] == 17
results['completed'] = True
(run / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(run / 'results.json')
