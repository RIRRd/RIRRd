#!/usr/bin/env python3
"""Small sequential comparison on the shared synthetic fixture, not a load qualification."""
import argparse
import concurrent.futures
import json
import hashlib
import os
import platform
from datetime import datetime, timezone
import math
from pathlib import Path
import statistics
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from differential_whois import endpoint, exchange, frame

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--reference', type=endpoint, required=True)
parser.add_argument('--candidate', type=endpoint, required=True)
parser.add_argument('--candidate-commit', required=True)
parser.add_argument('--requests', type=int, default=1000)
parser.add_argument('--concurrency', type=int, default=8)
parser.add_argument('--first', choices=['reference','candidate'], default='reference')
parser.add_argument('--candidate-binary', type=Path)
parser.add_argument('--reference-workers', type=int, default=1)
args = parser.parse_args()
if not 1 <= args.requests <= 100000 or not 1 <= args.concurrency <= 128 or not 1 <= args.reference_workers <= 128:
    parser.error('requests must be 1..100000; concurrency and reference-workers must be 1..128')
query = '!maut-num,AS64496'

def measure(address):
    def one(_):
        start = time.perf_counter()
        status, body = frame(exchange(address, query, 5))
        if status != 'A' or b'AS64496' not in body:
            raise ValueError('incorrect benchmark response')
        return (time.perf_counter() - start) * 1000
    for i in range(20):
        one(i)
    start = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        samples = sorted(pool.map(one, range(args.requests)))
    seconds = time.perf_counter() - start
    return {'requests': len(samples), 'seconds': seconds, 'queries_per_second': len(samples)/seconds,
            'p50_ms': statistics.median(samples), 'p95_ms': samples[math.ceil(.95*len(samples))-1],
            'p99_ms': samples[math.ceil(.99*len(samples))-1], 'max_ms': samples[-1]}

report = {
    'kind': 'synthetic six-object smoke; includes TCP connection per request',
    'candidate_commit': args.candidate_commit,
    'reference_commit': '8e8080dab744a348b7d0e6efcb53f403301dd075',
    'concurrency': args.concurrency,
    'first': args.first,
    'reference_workers': args.reference_workers,
    'started_at': datetime.now(timezone.utc).isoformat(),
    'host': {'cpus': len(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else os.cpu_count(), 'architecture': platform.machine(),
             'memory_bytes': os.sysconf('SC_PHYS_PAGES') * os.sysconf('SC_PAGE_SIZE')},
}
if args.candidate_binary:
    with args.candidate_binary.open('rb') as binary:
        report['candidate_binary_sha256'] = hashlib.file_digest(binary, 'sha256').hexdigest()
fixture = Path(__file__).resolve().parents[2] / 'docs/compatibility/differential-whois.rpsl'
report['fixture_sha256'] = hashlib.sha256(fixture.read_bytes()).hexdigest()
for label in [args.first, 'candidate' if args.first == 'reference' else 'reference']:
    report[label] = measure(getattr(args, label))
print(json.dumps(report, indent=2))
