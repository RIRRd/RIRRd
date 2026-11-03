#!/usr/bin/env python3
"""Independent community node submission queue; never fetch submitted endpoints."""
import argparse
from contextlib import contextmanager
import datetime
import hashlib
import hmac
import http.server
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import sqlite3
import threading
import time
import urllib.parse

MAX_BODY = 8192
MAX_PENDING = 500
FIELDS = {'name': 80, 'operator': 80, 'contact': 254, 'country': 60, 'country_code': 2,
          'region': 40, 'provider': 100, 'sponsor': 100, 'whois_host': 253,
          'https': 300, 'notes': 1500}


def public_host(host):
    host = host.strip().strip('[]').lower()
    try:
        if not ipaddress.ip_address(host).is_global:
            raise ValueError('Use a public address or hostname for the node.')
        return host
    except ValueError:
        if ':' in host or re.fullmatch(r'[\d.]+', host):
            raise ValueError('Use a public address or hostname for the node.')
    if len(host) > 253 or '.' not in host or host.endswith(('.localhost', '.local', '.internal', '.invalid')):
        raise ValueError('Use a public hostname for the node.')
    if not all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label) for label in host.split('.')):
        raise ValueError('The node hostname is invalid.')
    return host


def validate(data):
    if not isinstance(data, dict) or set(data) - set(FIELDS) - {'whois_port', 'authorised', 'website'}:
        raise ValueError('Unsupported submission fields.')
    if data.get('website') or data.get('authorised') is not True:
        raise ValueError('Confirm that you operate or are authorised to submit this node.')
    result = {}
    for name, limit in FIELDS.items():
        value = data.get(name, '')
        if not isinstance(value, str) or len(value) > limit or any(ord(c) < 32 and not (name == 'notes' and c == '\n') for c in value):
            raise ValueError('Check the length and formatting of ' + name.replace('_', ' ') + '.')
        result[name] = value.strip()
    for name in ['name', 'operator', 'contact', 'country', 'country_code', 'region', 'provider', 'whois_host', 'https']:
        if not result[name]:
            raise ValueError('Complete ' + name.replace('_', ' ') + '.')
    if not re.fullmatch(r'[^\s@]+@[^\s@]+\.[^\s@]+', result['contact']):
        raise ValueError('Enter a valid operator contact email.')
    if not re.fullmatch('[A-Za-z]{2}', result['country_code']):
        raise ValueError('Enter a two-letter country code, such as GB.')
    result['country_code'] = result['country_code'].upper()
    result['whois_host'] = public_host(result['whois_host'])
    port = data.get('whois_port', 43)
    if not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535:
        raise ValueError('WHOIS port must be between 1 and 65535.')
    result['whois_port'] = port
    url = urllib.parse.urlsplit(result['https'])
    if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
        raise ValueError('Use a public HTTPS URL without credentials or query parameters.')
    public_host(url.hostname)
    try:
        if url.port not in (None, 443):
            raise ValueError('Use the public HTTPS endpoint on port 443.')
    except ValueError:
        raise ValueError('Use the public HTTPS endpoint on port 443.')
    return result


class Queue:
    def __init__(self, root):
        self.root = Path(root)
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.root.chmod(0o700)
        key = self.root / 'rate-key'
        try:
            with key.open('xb') as out:
                out.write(secrets.token_bytes(32))
            key.chmod(0o600)
        except FileExistsError:
            pass
        self.key = key.read_bytes()
        self.database = self.root / 'submissions.sqlite3'
        with self.connect() as db:
            db.execute('CREATE TABLE IF NOT EXISTS submissions (id TEXT PRIMARY KEY, created INTEGER NOT NULL, client TEXT NOT NULL, status TEXT NOT NULL, payload TEXT NOT NULL)')
            db.execute('CREATE INDEX IF NOT EXISTS submission_rate ON submissions(client,created)')
        self.database.chmod(0o600)
        self.purge()

    @contextmanager
    def connect(self):
        db = sqlite3.connect(self.database, timeout=2)
        try:
            db.execute('PRAGMA secure_delete=ON')
            with db:
                yield db
        finally:
            db.close()

    def purge(self, now=None):
        now = int(time.time()) if now is None else now
        with self.connect() as db:
            db.execute('DELETE FROM submissions WHERE created < ?', (now - 90 * 86400,))

    def submit(self, data, client, now=None):
        payload = validate(data)
        now = int(time.time()) if now is None else now
        address = ipaddress.ip_address(client)
        group = str(address) if address.version == 4 else str(ipaddress.ip_network(str(address) + '/64', strict=False))
        fingerprint = hmac.new(self.key, (str(now // 86400) + ':' + group).encode(), hashlib.sha256).hexdigest()
        identity = secrets.token_hex(12)
        with self.connect() as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('DELETE FROM submissions WHERE created < ?', (now - 90 * 86400,))
            if db.execute('SELECT COUNT(*) FROM submissions WHERE client=? AND created>?', (fingerprint, now - 3600)).fetchone()[0] >= 3:
                raise OverflowError('Submission limit reached. Please try again later.')
            if db.execute("SELECT COUNT(*) FROM submissions WHERE status='pending'").fetchone()[0] >= MAX_PENDING:
                raise OverflowError('The review queue is full. Please try again later.')
            db.execute('INSERT INTO submissions VALUES (?,?,?,?,?)', (identity, now, fingerprint, 'pending', json.dumps(payload)))
        return identity


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address, queue, origin):
        self.queue = queue
        self.origin = origin
        self.slots = threading.BoundedSemaphore(8)
        super().__init__(address, Handler)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, client_address)
        except Exception:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = 'RIRRd-community'
    sys_version = ''
    def setup(self):
        super().setup()
        self.connection.settimeout(8)

    def log_message(self, *_):
        pass

    def respond(self, code, value):
        body = json.dumps(value).encode()
        self.send_response(code)
        for key, value in [('Content-Type', 'application/json'), ('Content-Length', str(len(body))), ('Cache-Control', 'no-store'), ('X-Content-Type-Options', 'nosniff'), ('Connection', 'close')]:
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def do_GET(self):
        self.respond(200 if self.path == '/health' else 404, {'status': 'ready'} if self.path == '/health' else {'error': 'Not found'})

    def do_POST(self):
        if self.path != '/community/node-submissions':
            return self.respond(404, {'error': 'Not found'})
        if self.headers.get('Origin') != self.server.origin:
            return self.respond(403, {'error': 'Submit from the community website.'})
        if self.headers.get('Transfer-Encoding') or len(self.headers.get_all('Content-Length', [])) != 1:
            return self.respond(400, {'error': 'Invalid request framing.'})
        if self.headers.get('Content-Type', '').split(';')[0] != 'application/json':
            return self.respond(415, {'error': 'Use JSON submission data.'})
        try:
            length = int(self.headers['Content-Length'])
            if not 0 < length <= MAX_BODY:
                return self.respond(413, {'error': 'Submission is too large.'})
            body = self.rfile.read(length)
            if len(body) != length:
                return self.respond(400, {'error': 'Incomplete submission.'})
            data = json.loads(body)
            identity = self.server.queue.submit(data, self.headers.get('X-RIRRd-Client', self.client_address[0]))
            return self.respond(202, {'id': identity, 'status': 'pending', 'message': 'Submitted for review. Your contact details are private; the node is not published automatically.'})
        except OverflowError as error:
            return self.respond(429, {'error': str(error)})
        except (ValueError, TypeError, UnicodeError, RecursionError):
            return self.respond(422, {'error': 'Check the required fields, public endpoints and operator authorisation.'})
        except (OSError, sqlite3.Error):
            return self.respond(503, {'error': 'Submission storage is temporarily unavailable. Please retry later.'})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state', type=Path, default=Path('/var/lib/rirrd-community'))
    commands = parser.add_subparsers(dest='command', required=True)
    serve = commands.add_parser('serve')
    serve.add_argument('--port', type=int, default=9282)
    serve.add_argument('--origin', default='https://irrd.tools')
    commands.add_parser('list')
    review = commands.add_parser('review')
    review.add_argument('id')
    review.add_argument('--decision', choices=['approve', 'reject'], required=True)
    review.add_argument('--directory', type=Path)
    review.add_argument('--verification-note')
    args = parser.parse_args()
    queue = Queue(args.state)
    if args.command == 'serve':
        def cleanup():
            while True:
                time.sleep(3600)
                try:
                    queue.purge()
                except (OSError, sqlite3.Error):
                    pass  # Retry at the next interval without logging contact details.
        threading.Thread(target=cleanup, daemon=True).start()
        Server(('127.0.0.1', args.port), queue, args.origin).serve_forever()
    elif args.command == 'list':
        with queue.connect() as db:
            for identity, created, status, payload in db.execute('SELECT id,created,status,payload FROM submissions ORDER BY created'):
                print(json.dumps({'id': identity, 'created': created, 'status': status, **json.loads(payload)}))
    else:
        if args.decision == 'approve' and (not args.directory or not args.verification_note):
            parser.error('Approval requires a directory and a completed verification note.')
        with queue.connect() as db:
            row = db.execute('SELECT payload,status FROM submissions WHERE id=?', (args.id,)).fetchone()
            if not row or row[1] != 'pending':
                parser.error('Pending submission not found.')
            if args.decision == 'approve':
                item = json.loads(row[0])
                directory = json.loads(args.directory.read_text())
                if any(n.get('id') == args.id for n in directory['nodes']):
                    parser.error('Node already exists in this directory.')
                host = item['whois_host']
                endpoint = ('[' + host + ']' if ':' in host else host) + ':' + str(item['whois_port'])
                directory['nodes'].append({k: item[k] for k in ['name','operator','country','country_code','region','provider','sponsor','https']})
                directory['nodes'][-1].update(id=args.id, whois=endpoint, status='verified', verified_at=datetime.datetime.now(datetime.timezone.utc).isoformat(), verification_scope=args.verification_note)
                temporary = args.directory.with_suffix('.next')
                temporary.write_text(json.dumps(directory, indent=2) + '\n')
                os.replace(temporary, args.directory)
            db.execute('UPDATE submissions SET status=? WHERE id=?', ('approved' if args.decision == 'approve' else 'rejected', args.id))
            print('Review recorded. Regenerate and deploy the community directory to publish approved changes.')


if __name__ == '__main__':
    main()
