#!/usr/bin/env python3
"""Build a complete candidate database without touching the serving generation."""
import argparse
import collections
import datetime
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.parse
import urllib.request

MIB = 1024 * 1024
MAX_DOWNLOAD = 128 * MIB
MAX_SOURCE = 1536 * MIB
MAX_RECORD = 4 * MIB


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def download(url, target):
    if urllib.parse.urlsplit(url).scheme != 'https':
        raise ValueError('Only explicit HTTPS snapshot endpoints are enabled')
    started = time.monotonic()
    request = urllib.request.Request(url, headers={'User-Agent': 'RIRRd-experimental-mirror/0.1'})
    total = 0
    digest = hashlib.sha256()
    with urllib.request.urlopen(request, timeout=30) as response, target.open('xb') as out:
        if urllib.parse.urlsplit(response.url).scheme != 'https':
            raise ValueError('Insecure download redirect')
        while block := response.read(65536):
            total += len(block)
            if total > MAX_DOWNLOAD or time.monotonic() - started > 300:
                raise ValueError('Snapshot compressed byte/time budget exceeded')
            out.write(block)
            digest.update(block)
        modified = response.headers.get('Last-Modified')
    return {'url': url, 'bytes': total, 'sha256': digest.hexdigest(),
            'last_modified': modified, 'fetched_at': now()}


def records(path):
    total, record = 0, bytearray()
    with gzip.open(path, 'rb') as stream:
        while line := stream.readline(MAX_RECORD + 1):
            total += len(line)
            if total > MAX_SOURCE or len(line) > MAX_RECORD:
                raise ValueError('Decompressed snapshot budget exceeded')
            if not line.strip():
                if record:
                    yield bytes(record)
                    record.clear()
            elif record or not line.startswith((b'%', b'#')):
                record.extend(line)
                if len(record) > MAX_RECORD:
                    raise ValueError('Snapshot object exceeds 4 MiB')
        if record:
            yield bytes(record)


def normalize(paths, target, source, classes, encoding):
    counts, excluded = collections.Counter(), collections.Counter()
    with target.open('x', encoding='utf-8') as output:
        for path in paths:
            for raw in records(path):
                text = raw.decode(encoding)
                lines = text.splitlines()
                fields = []
                for line in lines:
                    if line.startswith(('%', '#', ' ', '\t', '+')):
                        continue
                    name, sep, value = line.partition(':')
                    if sep:
                        fields.append((name.lower(), value.split('#')[0].strip()))
                if not fields:
                    continue
                cls = fields[0][0]
                if cls not in classes:
                    excluded[cls] += 1
                    continue
                sources = [v.upper() for k, v in fields if k == 'source']
                if sources != [source]:
                    raise ValueError('Snapshot source identity mismatch')
                if any(k in ('password', 'override', 'api-key', 'auth', 'certif') for k, _ in fields):
                    raise ValueError('Unexpected credential field in routing-class snapshot')
                output.write('\n'.join(lines) + '\n\n')
                counts[cls] += 1
    if not counts:
        raise ValueError('No routing objects in snapshot')
    if target.stat().st_size > MAX_SOURCE:
        raise ValueError('Combined source exceeds normalized byte budget')
    return dict(counts), dict(excluded)


def run(args):
    catalog = json.loads(args.catalog.read_text())
    enabled = [row for row in catalog['sources'] if row['state'] == 'enabled']
    if shutil.disk_usage(args.output.parent).free < 12 * 1024**3:
        raise ValueError('At least 12 GiB free staging space is required')
    args.output.mkdir(mode=0o700)
    snapshot_dir = args.output / 'snapshots'
    snapshot_dir.mkdir(mode=0o700)
    database = args.output / 'registry.redb'
    config = args.output / 'irrd.toml'
    config.write_text('database = ' + json.dumps(str(database.resolve())) + '''
database_cache_mib = 64
worker_threads = 2
max_blocking_threads = 4
sources = ''' + json.dumps([s['source'] for s in enabled]) + '''
http_sources = ''' + json.dumps([s['source'] for s in enabled if s.get('http_access', True)]) + '''
listen = "[::]:43"
http_listen = "127.0.0.1:9280"
max_connections = 128
max_connections_per_ip = 8
io_timeout_seconds = 10
max_snapshot_bytes = 1610612736
[node]
name = "RIRRd public test registry"
''')
    report = {'schema_version': 1, 'built_at': now(), 'refresh_mode': catalog['refresh'],
              'binary_sha256': hashlib.file_digest(args.binary.open('rb'), 'sha256').hexdigest(),
              'classes': catalog['classes'], 'sources': [], 'total_objects': 0}
    for entry in catalog['sources']:
        if entry['state'] != 'enabled':
            report['sources'].append(entry)
            continue
        source = entry['source']
        files, downloads = [], []
        for i, url in enumerate(entry['urls']):
            path = snapshot_dir / f'{source}-{i}.gz'
            downloads.append(download(url, path))
            files.append(path)
        path = snapshot_dir / f'{source}.rpsl'
        counts, excluded = normalize(files, path, source, set(catalog['classes']), entry['encoding'])
        start = time.monotonic()
        result = subprocess.run([str(args.binary), '--config', str(config), 'import', 'snapshot',
                                 '--source', source, '--file', str(path)],
                                capture_output=True, text=True, timeout=1800)
        if result.returncode:
            # Diagnostics stay in private staging, never returned by the public API.
            (args.output / f'{source}-import-error.txt').write_text(result.stderr)
            raise ValueError(f'{source} strict import failed; candidate not published')
        imported = json.loads(result.stdout)
        if imported['objects'] != sum(counts.values()):
            raise ValueError('Imported object count does not match normalized snapshot')
        row = {**entry, 'state': 'loaded', 'objects': imported['objects'], 'classes': counts,
               'excluded_classes': excluded, 'downloads': downloads, 'imported_at': now(),
               'import_seconds': time.monotonic() - start}
        warnings = []
        for line in result.stderr.splitlines():
            try:
                event = json.loads(line)
                if event.get('level') == 'WARN':
                    warnings.append(event.get('fields', {}))
            except ValueError:
                continue
        row['skipped_lookup_values'] = sum(int(event.get('skipped_lookup_values', 0)) for event in warnings)
        row['lookup_note'] = 'Malformed optional lookup values remain in object text but are not indexed; this is not authoritative schema validation.'
        report['sources'].append(row)
        report['total_objects'] += imported['objects']
        print(json.dumps({'source': source, 'objects': imported['objects'],
                          'import_seconds': row['import_seconds']}), flush=True)
    verified = subprocess.run([str(args.binary), '--config', str(config), 'database', 'verify'],
                              capture_output=True, text=True, check=True, timeout=1800)
    if json.loads(verified.stdout)['objects'] != report['total_objects']:
        raise ValueError('Complete candidate integrity/count check failed')
    with args.binary.open('rb') as executable:
        if hashlib.file_digest(executable, 'sha256').hexdigest() != report['binary_sha256']:
            raise ValueError('Binary changed during snapshot build; candidate not published')
    report['verified_at'] = now()
    report['database_bytes'] = database.stat().st_size
    (args.output / 'sources.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'verified_objects': report['total_objects'], 'database_bytes': report['database_bytes']}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--catalog', type=Path, default=Path(__file__).with_name('sources.json'))
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New private generation directory')
    run(parser.parse_args())
