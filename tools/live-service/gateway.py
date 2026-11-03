#!/usr/bin/env python3
"""Bounded read-only HTTP gateway and measured telemetry for a local RIRRd."""
import argparse
import collections
import datetime
import http.client
import http.server
import ipaddress
import json
import math
from pathlib import Path
import socket
import subprocess
import threading
import time
import urllib.parse

MAX_BODY = 16 * 1024
MAX_RESPONSE = 2 * 1024 * 1024
NODE = json.loads(Path(__file__).with_name('node.json').read_text())


class State:
    def __init__(self, generation, backend_port):
        self.generation, self.backend_port = generation, backend_port
        self.lock = threading.Lock()
        self.clients = collections.OrderedDict()
        self.requests = self.errors = 0
        self.durations = collections.deque(maxlen=512)
        self.started = time.monotonic()
        self.telemetry = {}
        self.previous_cpu = None

    def allow(self, client, clock=None):
        clock = time.monotonic() if clock is None else clock
        address = ipaddress.ip_address(client)
        # Group IPv6 privacy addresses per /64 rather than granting each a burst.
        key = str(address) if address.version == 4 else str(ipaddress.ip_network(f'{address}/64', strict=False))
        with self.lock:
            tokens, previous = self.clients.pop(key, (20., clock))
            tokens = min(20., tokens + max(0., clock - previous) * 2)
            allowed = tokens >= 1
            self.clients[key] = (tokens - 1 if allowed else tokens, clock)
            while len(self.clients) > 4096:
                self.clients.popitem(last=False)
            return allowed

    def record(self, elapsed, error):
        with self.lock:
            self.requests += 1
            self.errors += int(error)
            self.durations.append(elapsed * 1000)

    def collect(self):
        try:
            report = json.loads((self.generation / 'sources.json').read_text())
            catalog_path = Path(__file__).with_name('sources.json')
            if catalog_path.exists():
                catalog = {row['source']: row for row in json.loads(catalog_path.read_text())['sources']}
                report['sources'] = [row if row['state'] == 'loaded' else {**row, 'reason':
                    ('Public feed available; a verified database generation is being prepared.'
                     if catalog.get(row['source'], {}).get('state') == 'enabled'
                     else catalog.get(row['source'], {}).get('reason', row.get('reason')))}
                    for row in report['sources']]
            health = http.client.HTTPConnection('127.0.0.1', self.backend_port, timeout=2)
            try:
                health.request('POST', '/graphql/', body=b'{"query":"{ databaseStatus { source } }"}',
                               headers={'Host': '127.0.0.1', 'Content-Type': 'application/json'})
                response = health.getresponse()
                body = response.read(MAX_BODY)
                actual = json.loads(body)
                expected = {row['source'] for row in report['sources'] if row['state'] == 'loaded' and row.get('http_access', True)}
                if response.status != 200 or {row['source'] for row in actual['data']['databaseStatus']} != expected:
                    raise ValueError('Local query service is not ready')
            finally:
                health.close()
            mirror_data = {}
            monitor = http.client.HTTPConnection('127.0.0.1', self.backend_port, timeout=2)
            try:
                monitor.request('GET', '/ui/mirrors.json', headers={'Host': '127.0.0.1'})
                response = monitor.getresponse()
                data = response.read(128 * 1024)
                if response.status == 200:
                    mirror_data = json.loads(data)
                elif response.status != 404:  # Allow a rolling upgrade from the older daemon.
                    raise ValueError('Mirror status is unavailable')
            finally:
                monitor.close()
            cgroup = Path('/sys/fs/cgroup/system.slice/rirrd-live.service')
            cpu = dict(line.split() for line in (cgroup / 'cpu.stat').read_text().splitlines())
            current_cpu, clock = int(cpu['usage_usec']), time.monotonic()
            cpu_percent = None
            if self.previous_cpu:
                cpu_percent = max(0., (current_cpu - self.previous_cpu[0]) / (clock - self.previous_cpu[1]) / 10000)
            self.previous_cpu = current_cpu, clock
            pid = int(subprocess.check_output(['systemctl', 'show', '-p', 'MainPID', '--value', 'rirrd-live.service'], timeout=2))
            process = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
            rss = int(process['VmRSS'].split()[0]) * 1024
            stat = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
            import os
            uptime = float(Path('/proc/uptime').read_text().split()[0]) - int(stat[19]) / os.sysconf('SC_CLK_TCK')
            snapshot = {'observed_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        'service': 'online', 'process_rss_bytes': rss,
                        'cgroup_memory_bytes': int((cgroup / 'memory.current').read_text()),
                        'cgroup_memory_peak_bytes': int((cgroup / 'memory.peak').read_text()),
                        'cpu_percent_one_core': cpu_percent, 'uptime_seconds': max(0, uptime),
                        'database_bytes': (self.generation / 'registry.redb').stat().st_size,
                        'total_objects': mirror_data.get('total_objects', report['total_objects']),
                        'snapshot_total_objects': report.get('snapshot_total_objects', report['total_objects']),
                        'snapshot_verified_at': report['verified_at'], 'refresh_mode': report['refresh_mode'],
                        'classes': report['classes'], 'class_counts': mirror_data.get('class_counts'),
                        'sources': [{**row, 'http_whois_access': row['state'] == 'loaded', 'graphql_access': row.get('http_access', True), 'mirror': mirror_data.get('sources', {}).get(row['source'])}
                                    for row in report['sources']]}
            connections = 0
            for table in ('tcp', 'tcp6'):
                for row in Path('/proc/net/' + table).read_text().splitlines()[1:]:
                    fields = row.split()
                    if fields[1].split(':')[1] == '002B' and fields[3] == '01':
                        connections += 1
            snapshot['whois_connections'] = connections
            release_file = Path('/usr/local/lib/rirrd-live/release.json')
            if release_file.is_file():
                snapshot['release'] = json.loads(release_file.read_text())
            refresh_state = self.generation.parent / 'last-refresh.json'
            # generation is the root-owned 'current' symlink, not its resolved target.
            if refresh_state.is_file():
                snapshot['last_refresh'] = json.loads(refresh_state.read_text())
            with self.lock:
                self.telemetry = snapshot
        except (OSError, ValueError, KeyError, http.client.HTTPException, subprocess.SubprocessError):
            with self.lock:
                self.telemetry = {**self.telemetry, 'service': 'unavailable',
                                  'error': 'Live telemetry unavailable; do not assume source freshness.'}

    def stats(self):
        with self.lock:
            values = sorted(self.durations)
            return {**self.telemetry, 'gateway_uptime_seconds': time.monotonic() - self.started,
                    'http_queries': self.requests, 'http_errors': self.errors,
                    'http_recent_sample_count': len(values),
                    'http_recent_p95_ms': values[max(0, math.ceil(len(values) * .95) - 1)] if values else None,
                    'counter_scope': 'HTTP gateway attempts since gateway restart; excludes TCP and monitoring. Latency includes proxy overhead; most recent 512 attempts.'}


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 32

    def __init__(self, address, state):
        self.state = state
        self.slots = threading.BoundedSemaphore(16)
        super().__init__(address, Handler)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            try:
                request.sendall(b'HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n')
            finally:
                self.shutdown_request(request)
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
    server_version = 'RIRRd-gateway'
    sys_version = ''

    def setup(self):
        super().setup()
        self.connection.settimeout(12)

    def log_message(self, *_args):
        pass  # Do not persist visitor addresses or query text.

    def respond(self, status, data=b'', content_type='application/json'):
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Connection', 'close')
        if status == 429:
            self.send_header('Retry-After', '1')
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def do_GET(self):
        self.dispatch()

    def do_POST(self):
        self.dispatch()

    def dispatch(self):
        path = urllib.parse.urlsplit(self.path)
        if path.scheme or path.netloc or self.headers.get('Transfer-Encoding'):
            return self.respond(400)
        if len(self.path) > 4096:
            return self.respond(414)
        state = self.server.state
        if self.command == 'GET' and path.path == '/dashboard/live.json' and not path.query:
            return self.respond(200, json.dumps(state.stats()).encode())
        if self.command == 'GET' and path.path == '/dashboard/meta.json' and not path.query:
            stats = state.stats()
            loaded = [s['source'] for s in stats.get('sources', []) if s['state'] == 'loaded']
            return self.respond(200, json.dumps({'mode': 'live', 'public_service': True,
                'source_revision': stats.get('release', {}).get('source_commit'),
                'version': '0.1.0', 'node': {**NODE, 'sponsor': NODE['provider']},
                'sources': loaded, 'whois_listen': 'rirrd.co.uk:43',
                'http_sources': [s['source'] for s in stats.get('sources', []) if s['state'] == 'loaded' and s.get('http_access', True)],
                'http_whois_sources': [s['source'] for s in stats.get('sources', []) if s['state'] == 'loaded'],
                'max_connections': 128, 'database_cache_mib': 64}).encode())
        permitted = (self.command == 'GET' and path.path in ('/v1/whois/', '/v1/rpki/status', '/v1/rpki/validate', '/v1/irr/objects')) or (
            self.command == 'POST' and path.path == '/graphql/' and not path.query)
        if not permitted:
            return self.respond(404)
        # Caddy overwrites this header from its immediate public connection.
        client = self.headers.get('X-RIRRd-Client', self.client_address[0])
        try:
            if not state.allow(client):
                return self.respond(429)
        except ValueError:
            return self.respond(400)
        lengths = self.headers.get_all('Content-Length', [])
        try:
            if len(lengths) > 1:
                raise ValueError()
            length = int(lengths[0]) if lengths else 0
            if length < 0 or length > MAX_BODY:
                return self.respond(413)
            if self.command == 'GET' and length:
                return self.respond(400)
            body = self.rfile.read(length)
            if len(body) != length:
                return self.respond(400)
        except (ValueError, socket.timeout):
            return self.respond(400)
        started = time.monotonic()
        connection = http.client.HTTPConnection('127.0.0.1', state.backend_port, timeout=10)
        error = True
        try:
            connection.request(self.command, self.path, body=body if length else None,
                               headers={'Host': '127.0.0.1', 'Content-Type': 'application/json'})
            response = connection.getresponse()
            data = response.read(MAX_RESPONSE + 1)
            if len(data) > MAX_RESPONSE:
                return self.respond(502, b'{"error":"Response exceeds public gateway limit"}')
            error = response.status >= 400
            self.respond(response.status, data, response.getheader('Content-Type', 'application/json'))
        except (OSError, http.client.HTTPException):
            self.respond(503, b'{"error":"Registry temporarily unavailable"}')
        finally:
            if path.path != '/v1/rpki/status':
                state.record(time.monotonic() - started, error)
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--generation', type=Path, required=True)
    parser.add_argument('--port', type=int, default=9281)
    parser.add_argument('--backend-port', type=int, default=9280)
    args = parser.parse_args()
    state = State(args.generation, args.backend_port)
    state.collect()
    def collector():
        while True:
            time.sleep(5)
            state.collect()
    threading.Thread(target=collector, daemon=True).start()
    Server(('127.0.0.1', args.port), state).serve_forever()


if __name__ == '__main__':
    main()
