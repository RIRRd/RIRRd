#!/usr/bin/env python3
"""Build a complete candidate database without touching the serving generation."""
import argparse
import collections
import datetime
import gzip
import hashlib
import json
import ipaddress
import os
import re
from pathlib import Path
import shutil
import subprocess
import sqlite3
import tempfile
import time
import urllib.parse
import urllib.request

MIB = 1024 * 1024
MAX_DOWNLOAD = 128 * MIB
MAX_SOURCE = 1536 * MIB
MAX_RECORD = 4 * MIB


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def check_transport(url, allow_ftp=False):
    endpoint = urllib.parse.urlsplit(url)
    if endpoint.username is not None or endpoint.password is not None:
        raise ValueError('Credentials do not belong in public snapshot URLs')
    if endpoint.scheme == 'https' and endpoint.hostname:
        return 'https'
    # Some official IRR publications support anonymous FTP only. This is an
    # explicit catalog choice, never a fallback from a failed TLS check.
    if (allow_ftp and endpoint.scheme == 'ftp'
            and endpoint.hostname == 'ftp.radb.net' and endpoint.port in (None, 21)
            and endpoint.path.startswith('/radb/dbase/') and not endpoint.query):
        return 'ftp'
    raise ValueError('Snapshot endpoint requires HTTPS or explicitly allowed official anonymous FTP')


def download(url, target, allow_ftp=False):
    transport = check_transport(url, allow_ftp)
    started = time.monotonic()
    request = urllib.request.Request(url, headers={'User-Agent': 'RIRRd-experimental-mirror/0.1'})
    total = 0
    digest = hashlib.sha256()
    with urllib.request.urlopen(request, timeout=30) as response, target.open('xb') as out:
        final = urllib.parse.urlsplit(response.url)
        if (transport == 'https' and final.scheme != 'https') or (transport == 'ftp' and response.url != url):
            raise ValueError('Insecure download redirect')
        while block := response.read(65536):
            total += len(block)
            if total > MAX_DOWNLOAD or time.monotonic() - started > 300:
                raise ValueError('Snapshot compressed byte/time budget exceeded')
            out.write(block)
            digest.update(block)
        modified = response.headers.get('Last-Modified')
    return {'url': url, 'bytes': total, 'sha256': digest.hexdigest(), 'transport': transport,
            'transport_authenticated': transport == 'https',
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


def normalize(paths, target, source, classes, encoding, rejection_counts=None):
    counts, excluded = collections.Counter(), collections.Counter()
    rejected = {} if rejection_counts is None else rejection_counts
    duplicates = 0
    # Published ARIN snapshots can contain the same canonical route identity
    # with differently cased origins. Match pinned IRRd's last-upsert-wins
    # snapshot behavior without weakening the normal CLI's duplicate rejection.
    with tempfile.TemporaryDirectory(prefix='normalizing-', dir=target.parent) as work:
      connection = sqlite3.connect(Path(work) / 'identities.sqlite')
      connection.execute('PRAGMA cache_size=-8192')
      connection.execute('CREATE TABLE identities (class TEXT, identity TEXT, offset INTEGER, length INTEGER, ordinal INTEGER, PRIMARY KEY(class, identity))')
      ordinal = 0
      with (Path(work) / 'records').open('w+b') as spool:
        for path in paths:
            for raw in records(path):
                text = raw.decode(encoding)
                # RPSL records use LF/CRLF, not Unicode paragraph separators
                # that can legitimately occur inside a published description.
                lines = text.split('\n')
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
                # Do not repair malformed member names by deleting control
                # characters: that could invent a different routing policy.
                # Withhold the complete object and report its omission instead.
                if any((ord(c) < 32 and c not in '\n\r\t') or ord(c) == 127 for c in text):
                    reason = 'unsafe_ascii_control'
                    rejected[reason] = rejected.get(reason, 0) + 1
                    continue
                identity = fields[0][1].upper()
                if cls in ('route', 'route6'):
                    origins = [v for k, v in fields if k == 'origin']
                    if len(origins) != 1 or not origins[0].upper().startswith('AS'):
                        raise ValueError('Invalid route origin identity')
                    network, length = identity.split('/')
                    if cls == 'route':
                        # RPSL accepts decimal IPv4 octets with leading zeroes;
                        # Python deliberately rejects their ambiguous spelling.
                        octets = network.split('.')
                        if len(octets) != 4 or not all(o.isascii() and o.isdigit() for o in octets):
                            raise ValueError('Invalid IPv4 route identity')
                        network = '.'.join(str(int(o)) for o in octets)
                    identity = str(ipaddress.ip_network(network + '/' + str(int(length)))).upper() + 'AS' + str(int(origins[0][2:]))
                elif cls == 'aut-num':
                    if not identity.startswith('AS'):
                        raise ValueError('Invalid aut-num identity')
                    identity = 'AS' + str(int(identity[2:]))
                previous = connection.execute('SELECT 1 FROM identities WHERE class=? AND identity=?', (cls, identity)).fetchone()
                if previous:
                    duplicates += 1
                else:
                    counts[cls] += 1
                record = ('\n'.join(lines) + '\n\n').encode('utf-8')
                offset = spool.tell()
                if offset + len(record) > MAX_SOURCE:
                    raise ValueError('Combined source exceeds normalized byte budget')
                spool.write(record)
                connection.execute('INSERT OR REPLACE INTO identities VALUES (?,?,?,?,?)', (cls, identity, offset, len(record), ordinal))
                ordinal += 1
        connection.commit()
        with target.open('xb') as output:
            for offset, length in connection.execute('SELECT offset,length FROM identities ORDER BY ordinal'):
                spool.seek(offset)
                output.write(spool.read(length))
      connection.close()
    if not counts:
        raise ValueError('No routing objects in snapshot')
    if target.stat().st_size > MAX_SOURCE:
        raise ValueError('Combined source exceeds normalized byte budget')
    return dict(counts), dict(excluded), duplicates


def run(args):
    catalog = json.loads(args.catalog.read_text())
    names = [row['source'] for row in catalog['sources']]
    if len(names) != len(set(names)) or not all(re.fullmatch('[A-Z][A-Z0-9_-]{0,31}', name) for name in names):
        raise ValueError('Catalog source names must be safe, unique canonical identifiers')
    enabled = [row for row in catalog['sources'] if row['state'] == 'enabled']
    if shutil.disk_usage(args.output.parent).free < 12 * 1024**3:
        raise ValueError('At least 12 GiB free staging space is required')
    inputs = {'schema_version': 2, 'catalog_sha256': sha256(args.catalog), 'binary_sha256': sha256(args.binary)}
    completed = {}
    if args.resume:
        if args.output.is_symlink() or json.loads((args.output / 'build-inputs.json').read_text()) != inputs:
            raise ValueError('Resume requires the same private candidate, binary and source catalog')
        completed = json.loads((args.output / 'progress.json').read_text())['completed']
        subprocess.run([str(args.binary), '--config', str(args.output / 'irrd.toml'), 'database', 'verify'],
                       check=True, capture_output=True, text=True, timeout=1800)
    else:
        args.output.mkdir(mode=0o700)
        write_json(args.output / 'build-inputs.json', inputs)
        write_json(args.output / 'progress.json', {'completed': {}})
    snapshot_dir = args.output / 'snapshots'
    snapshot_dir.mkdir(mode=0o700, exist_ok=args.resume)
    database = args.output / 'registry.redb'
    config = args.output / 'irrd.toml'
    config_text = 'database = ' + json.dumps(str(database.resolve())) + '''
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
'''
    if args.resume:
        if config.read_text() != config_text:
            raise ValueError('Candidate configuration changed; refusing to resume')
    else:
        config.write_text(config_text)
    report = {'schema_version': 1, 'built_at': now(), 'refresh_mode': catalog['refresh'],
              'binary_sha256': inputs['binary_sha256'],
              'classes': catalog['classes'], 'sources': [], 'total_objects': 0}
    for entry in catalog['sources']:
        if entry['state'] != 'enabled':
            report['sources'].append(entry)
            continue
        source = entry['source']
        if source in completed:
            row = completed[source]
            if sha256(snapshot_dir / f'{source}.rpsl') != row['normalized_sha256']:
                raise ValueError('Completed normalized snapshot changed; candidate not published')
            report['sources'].append(row)
            report['total_objects'] += row['objects']
            print(json.dumps({'source': source, 'objects': row['objects'], 'resumed': True}), flush=True)
            continue
        files, downloads = [], []
        for i, url in enumerate(entry['urls']):
            path = snapshot_dir / f'{source}-{i}.gz'
            if args.resume:
                path.unlink(missing_ok=True)
            downloads.append(download(url, path, entry.get('allow_plaintext_ftp', False)))
            files.append(path)
        path = snapshot_dir / f'{source}.rpsl'
        if args.resume:
            path.unlink(missing_ok=True)
        rejections = {}
        counts, excluded, duplicates = normalize(files, path, source, set(catalog['classes']), entry['encoding'], rejections)
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
               'duplicate_identities_last_wins': duplicates,
               'quarantined_records': rejections,
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
        row['normalized_sha256'] = sha256(path)
        row['preparation_sha256'] = sha256(Path(__file__))
        completed[source] = row
        write_json(args.output / 'progress.json', {'completed': completed})
        report['sources'].append(row)
        report['total_objects'] += imported['objects']
        print(json.dumps({'source': source, 'objects': imported['objects'],
                          'import_seconds': row['import_seconds']}), flush=True)
    verified = subprocess.run([str(args.binary), '--config', str(config), 'database', 'verify'],
                              capture_output=True, text=True, check=True, timeout=1800)
    if json.loads(verified.stdout)['objects'] != report['total_objects']:
        raise ValueError('Complete candidate integrity/count check failed')
    if sha256(args.binary) != report['binary_sha256']:
        raise ValueError('Binary changed during snapshot build; candidate not published')
    report['verified_at'] = now()
    report['database_bytes'] = database.stat().st_size
    write_json(args.output / 'sources.json', report)
    print(json.dumps({'verified_objects': report['total_objects'], 'database_bytes': report['database_bytes']}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--catalog', type=Path, default=Path(__file__).with_name('sources.json'))
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New private generation directory')
    parser.add_argument('--resume', action='store_true', help='Resume a private candidate with identical catalog and binary inputs')
    run(parser.parse_args())
