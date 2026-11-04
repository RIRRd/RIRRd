"""Private, synthetic daemon fixture shared by command-line and browser checks."""
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[2]


class Registry:
    def __init__(self, binary=None):
        self.binary = Path(binary or os.environ['RIRRD_TEST_BINARY']).resolve()
        self.temp = tempfile.TemporaryDirectory(prefix='rirrd-contract-')
        self.root = Path(self.temp.name)
        self.config = self.root / 'irrd.toml'
        self.config.write_text('''database = "registry.redb"
sources = ["EXAMPLE", "SECOND"]
listen = "127.0.0.1:0"
http_listen = "127.0.0.1:0"
database_cache_mib = 8
worker_threads = 1
max_blocking_threads = 4
max_connections = 32
io_timeout_seconds = 2
max_snapshot_bytes = 262144
''', encoding='utf-8')
        self.child = None
        self.addresses = {}
        self.logs = []
        try:
            self.cli('import', 'snapshot', '--source', 'EXAMPLE', '--file', str(ROOT / 'tools/router-integration/fixture.rpsl'))
            fixture = self.root / 'second.rpsl'
            fixture.write_text('aut-num: AS64496\nas-name: SECONDARY\ndescr: Synthetic café\nsource: SECOND\n\nroute: 203.0.113.0/24\norigin: AS64496\nsource: SECOND\n', encoding='utf-8')
            self.cli('import', 'snapshot', '--source', 'SECOND', '--file', str(fixture))
        except BaseException:
            self.close()
            raise

    def cli(self, *args, success=True, config=None):
        result = subprocess.run([str(self.binary), '--config', str(config or self.config), *args], capture_output=True, timeout=45)
        if success and result.returncode:
            raise AssertionError('Fixture CLI failed: ' + result.stderr.decode('utf-8', errors='replace'))
        return result

    def start(self):
        assert self.child is None
        ready = threading.Event()
        self.addresses = {}
        self.child = subprocess.Popen([str(self.binary), '--config', str(self.config), 'server'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env={**os.environ, 'RUST_LOG':'info'})
        def consume():
            for line in self.child.stderr:
                if len(self.logs) < 100:
                    self.logs.append(line.decode('utf-8', errors='replace'))
                try:
                    fields = json.loads(line)['fields']
                    if fields.get('message') in ['WHOIS listening', 'GraphQL listening']:
                        host, port = fields['address'].rsplit(':', 1)
                        self.addresses[fields['message']] = (host, int(port))
                        if len(self.addresses) == 2:
                            ready.set()
                except (ValueError, KeyError):
                    pass
        self.reader = threading.Thread(target=consume, daemon=True)
        self.reader.start()
        if not ready.wait(20):
            self.stop()
            raise AssertionError('Fixture listeners did not start: ' + ''.join(self.logs))
        return self

    @property
    def http(self):
        return self.addresses['GraphQL listening']

    @property
    def tcp(self):
        return self.addresses['WHOIS listening']

    def request(self, path, method='GET', body=None, headers=None):
        connection = http.client.HTTPConnection(*self.http, timeout=5)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            return response.status, response.read(2 * 1024 * 1024), dict(response.getheaders())
        finally:
            connection.close()

    def wire(self, data, http=False):
        with socket.create_connection(self.http if http else self.tcp, timeout=5) as conn:
            conn.settimeout(5)
            conn.sendall(data)
            chunks = []
            size = 0
            while True:
                part = conn.recv(65536)
                if not part:
                    return b''.join(chunks)
                chunks.append(part)
                size += len(part)
                if size > 2 * 1024 * 1024:
                    raise AssertionError('Unbounded fixture response')

    def stop(self):
        if self.child:
            self.child.terminate()
            try:
                self.child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait(timeout=5)
            self.reader.join(timeout=3)
            self.child.stderr.close()
            self.child = None

    def close(self):
        self.stop()
        self.temp.cleanup()


def payload(reply):
    """Check exact IRRd byte framing before comparing a payload."""
    header, sep, tail = reply.partition(b'\n')
    if not sep or not header.startswith(b'A') or not header[1:].isdigit():
        raise AssertionError('Expected a length-framed answer, got ' + repr(reply[:100]))
    size = int(header[1:])
    if len(tail) != size + 2 or tail[size:] != b'C\n':
        raise AssertionError('Incorrect byte length or trailing success marker')
    return tail[:size]
