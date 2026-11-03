#!/usr/bin/env python3
"""Measure a local release binary on a deterministic synthetic WHOIS workload."""

import argparse
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import gzip
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import random
import shutil
import socket
import statistics
import subprocess
import tempfile
import threading
import time


BASE_ASN = 4_200_000_000
SOURCE = "BENCH"


def positive(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def record(asn, newline="\n"):
    return newline.join(
        (f"aut-num: AS{asn}", "as-name: BENCHMARK", f"source: {SOURCE}", "", "")
    ).encode("ascii")


def snapshots(directory, objects):
    plain = directory / "snapshot.txt"
    compressed = directory / "snapshot.txt.gz"
    with plain.open("wb") as output:
        for asn in range(BASE_ASN, BASE_ASN + objects):
            output.write(record(asn))
    with plain.open("rb") as source, compressed.open("wb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as archive:
            shutil.copyfileobj(source, archive)
    return plain, compressed


def command(binary, config, *args):
    return [str(binary), "--config", str(config), *map(str, args)]


def import_snapshot(binary, config, snapshot, database, expected_count, timeout):
    start = time.perf_counter()
    result = subprocess.run(
        command(binary, config, "import", "snapshot", "--source", SOURCE, "--file", snapshot),
        capture_output=True,
        text=True,
        check=True,
        timeout=timeout,
    )
    wall_seconds = time.perf_counter() - start
    stats = json.loads(result.stdout)
    if stats["objects"] != expected_count or stats["source"] != SOURCE:
        raise ValueError(f"import returned unexpected metadata: {stats}")
    return {
        "wall_seconds": wall_seconds,
        "daemon_seconds": stats["elapsed_seconds"],
        "database_file_bytes": database.stat().st_size,
    }


@contextmanager
def serve(binary, config, timeout):
    process = subprocess.Popen(
        command(binary, config, "server"),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        env={**os.environ, "RUST_LOG": "info"},
    )
    events = queue.Queue()
    last_lines = deque(maxlen=8)

    def read_logs():
        for line in process.stderr:
            last_lines.append(line.strip())
            try:
                fields = json.loads(line).get("fields", {})
            except json.JSONDecodeError:
                continue
            if fields.get("message") == "WHOIS listening":
                events.put(fields.get("address"))
        events.put(None)

    log_reader = threading.Thread(target=read_logs, daemon=True)
    log_reader.start()
    try:
        try:
            address = events.get(timeout=timeout)
        except queue.Empty:
            address = None
        if address is None:
            raise RuntimeError(f"WHOIS startup failed: {list(last_lines)}")
        host, port = address.rsplit(":", 1)
        if host != "127.0.0.1":
            raise RuntimeError(f"expected IPv4 loopback listener, got {address}")
        yield host, int(port)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        log_reader.join()
        process.stderr.close()


def query(address, asn, timeout):
    request = f"AS{asn}\r\n".encode("ascii")
    expected = record(asn, "\r\n")
    start = time.perf_counter_ns()
    with socket.create_connection(address, timeout=timeout) as connection:
        connection.sendall(request)
        response = bytearray()
        while chunk := connection.recv(len(expected) + 1 - len(response)):
            response.extend(chunk)
            if len(response) > len(expected):
                raise ValueError(f"unexpected WHOIS response length for AS{asn}")
    elapsed = time.perf_counter_ns() - start
    if response != expected:
        raise ValueError(f"unexpected WHOIS response for AS{asn}: {response!r}")
    return elapsed


def percentile(sorted_samples, percent):
    rank = math.ceil(len(sorted_samples) * percent / 100)
    return sorted_samples[rank - 1] / 1_000_000


def query_workload(address, args, concurrency, repetition):
    rng = random.Random(args.seed + repetition)
    workload = [BASE_ASN + rng.randrange(args.objects) for _ in range(args.queries)]

    def client(queries):
        return [query(address, asn, args.socket_timeout) for asn in queries]

    with ThreadPoolExecutor(max_workers=concurrency) as workers:
        warmup = workload[: args.warmup_queries]
        list(workers.map(client, (warmup[i::concurrency] for i in range(concurrency))))
        start = time.perf_counter()
        results = workers.map(client, (workload[i::concurrency] for i in range(concurrency)))
        samples = [sample for batch in results for sample in batch]
        wall_seconds = time.perf_counter() - start
    samples.sort()
    return {
        "queries": len(samples),
        "wall_seconds": wall_seconds,
        "queries_per_second": len(samples) / wall_seconds,
        "latency_ms": {
            **{f"p{percent}": percentile(samples, percent) for percent in (50, 90, 95, 99)},
            "max": samples[-1] / 1_000_000,
        },
    }


def run(args):
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="rust-irrd-benchmark-") as directory:
        workspace = Path(directory)
        config = workspace / "irrd.toml"
        database = workspace / "benchmark.redb"
        plain, compressed = snapshots(workspace, args.objects)
        config.write_text(
            f'database = "{database.name}"\n'
            f'sources = ["{SOURCE}"]\n'
            'listen = "127.0.0.1:0"\n'
            f"max_connections = {max(16, max(args.concurrency) * 2)}\n"
            f"io_timeout_seconds = {args.socket_timeout}\n"
            f"max_snapshot_bytes = {plain.stat().st_size}\n",
            encoding="utf-8",
        )
        imports = []
        queries = {str(count): [] for count in args.concurrency}
        for repetition in range(args.repetitions):
            database.unlink(missing_ok=True)
            timings = {}
            for label, snapshot in [
                ("fresh_database_plain", plain),
                ("existing_database_plain", plain),
                ("existing_database_gzip", compressed),
            ]:
                timings[label] = import_snapshot(
                    binary, config, snapshot, database, args.objects, args.process_timeout
                )
            imports.append(timings)
            with serve(binary, config, args.process_timeout) as address:
                for count in args.concurrency:
                    queries[str(count)].append(query_workload(address, args, count, repetition))

        return {
            "method": "synthetic exact ASN lookups on IPv4 loopback, one TCP connection per query",
            "profile": "supplied by caller; use a release build",
            "binary_sha256": digest(binary),
            "machine": {
                "os": platform.platform(),
                "processor": platform.processor() or platform.machine(),
                "logical_cpus": os.cpu_count(),
                "python": platform.python_version(),
            },
            "dataset": {
                "objects": args.objects,
                "plain_bytes": plain.stat().st_size,
                "plain_sha256": digest(plain),
                "gzip_bytes": compressed.stat().st_size,
                "gzip_sha256": digest(compressed),
            },
            "page_cache": "uncontrolled; fresh database does not imply a cold OS cache",
            "client_and_server": "same host",
            "seed": args.seed,
            "warmup_queries_per_run": args.warmup_queries,
            "repetitions": args.repetitions,
            "imports": imports,
            "whois": {
                count: {
                    "runs": runs,
                    "mean_queries_per_second": statistics.mean(
                        run["queries_per_second"] for run in runs
                    ),
                    "sample_stddev_queries_per_second": statistics.stdev(
                        run["queries_per_second"] for run in runs
                    )
                    if len(runs) > 1
                    else None,
                }
                for count, runs in queries.items()
            },
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--objects", type=positive, default=10_000)
    parser.add_argument("--queries", type=positive, default=10_000)
    parser.add_argument("--warmup-queries", type=positive, default=1_000)
    parser.add_argument("--concurrency", type=positive, nargs="+", default=[1, 8, 32])
    parser.add_argument("--repetitions", type=positive, default=3)
    parser.add_argument("--seed", type=int, default=2622)
    parser.add_argument("--process-timeout", type=positive, default=120)
    parser.add_argument("--socket-timeout", type=positive, default=5)
    args = parser.parse_args()
    if args.objects > 2**32 - BASE_ASN:
        parser.error("--objects exceeds the available generated ASN range")
    if args.warmup_queries > args.queries:
        parser.error("--warmup-queries must not exceed --queries")
    if len(set(args.concurrency)) != len(args.concurrency) or max(args.concurrency) > 128:
        parser.error("--concurrency must list unique counts at most 128")
    if args.socket_timeout > 300:
        parser.error("--socket-timeout must be at most 300 seconds")
    try:
        print(json.dumps(run(args), indent=2))
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"{error}\n{error.stderr}\n")


if __name__ == "__main__":
    main()
