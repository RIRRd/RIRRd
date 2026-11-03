#!/usr/bin/env python3
"""Compare IRRd-style exact-key WHOIS responses from two explicit TCP endpoints."""

import argparse
import math
from pathlib import Path
import re
import socket
import sys
from time import monotonic


DEFAULT_CASES = (
    ("A", "!maut-num,AS64496"),
    ("A", "!maut-num,as64496"),
    ("D", "!maut-num,AS64497"),
    ("D", "!mas-set,AS-DIFFERENTIAL-ABSENT"),
    ("A", "!mroute,192.0.2.0/24AS64496"),
    ("A", "!mroute,192.0.2.0/24 AS64496"),
    ("A", "!mroute,192.0.2.0/24-as64496"),
    ("D", "!mroute,192.0.2.0/24AS64497"),
    ("A", "!mroute6,2001:db8::/32AS64496"),
    ("A", "!mroute6,2001:db8::/32 as64496"),
    ("A", "!mroute6,2001:DB8::/32-AS64496"),
    ("D", "!mroute6,2001:db8::/32AS64497"),
    ("A", "!mmntner,DIFFERENTIAL-MNT"),
)
MAX_RESPONSE_BYTES = 256 * 1024
ATTRIBUTE_PADDING = re.compile(rb"^([a-zA-Z][a-zA-Z0-9-]*:)[ \t]*")
AUTH_PLACEHOLDER = re.compile(
    rb"auth:[ \t]*(?:CRYPT-PW|MD5-PW|BCRYPT-PW)[ \t]+DummyValue"
    rb"[ \t]+# Filtered for security",
    re.IGNORECASE,
)


def endpoint(value):
    host, delimiter, port = value.rpartition(":")
    if host.startswith("[") and host.endswith("]"):
        host = host[1:-1]
    elif ":" in host or "[" in host or "]" in host:
        host = ""
    if not host or not delimiter or not port.isascii() or not port.isdecimal():
        raise argparse.ArgumentTypeError("expected HOST:PORT or [IPv6]:PORT")
    port = int(port)
    if not 1 <= port <= 65535:
        raise argparse.ArgumentTypeError("expected a port from 1 to 65535")
    return host, port


def timeout_seconds(value):
    seconds = float(value)
    if not math.isfinite(seconds) or not 0 < seconds <= 60:
        raise argparse.ArgumentTypeError("timeout must be greater than 0 and at most 60 seconds")
    return seconds


def read_cases(path):
    cases = []
    for number, line in enumerate(path.read_text(encoding="ascii").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        expected, separator, query = line.partition("\t")
        if (
            not separator
            or expected not in {"A", "C", "D", "F"}
            or not query.startswith("!m")
            or not 2 <= len(query) <= 510
            or any(not 32 <= ord(char) <= 126 for char in query)
        ):
            raise ValueError(f"corpus line {number}: expected STATUS<TAB>!mquery")
        cases.append((expected, query))
    if not cases:
        raise ValueError("corpus contains no queries")
    return cases


def exchange(address, query, timeout):
    deadline = monotonic() + timeout
    with socket.create_connection(address, timeout) as conn:
        conn.settimeout(max(deadline - monotonic(), 0.001))
        conn.sendall(query.encode("ascii") + b"\r\n")
        response = bytearray()
        while True:
            remaining = deadline - monotonic()
            if remaining <= 0:
                raise TimeoutError("response deadline exceeded")
            conn.settimeout(remaining)
            chunk = conn.recv(min(8192, MAX_RESPONSE_BYTES + 1 - len(response)))
            if not chunk:
                return bytes(response)
            response.extend(chunk)
            if len(response) > MAX_RESPONSE_BYTES:
                raise ValueError("response exceeded byte limit")


def frame(raw):
    header, delimiter, remainder = raw.partition(b"\n")
    if not delimiter:
        raise ValueError("missing LF-terminated status")
    if header in {b"C", b"D"} and not remainder:
        return header.decode("ascii"), b""
    if header.startswith(b"F ") and len(header) > 2 and b"\r" not in header and not remainder:
        return "F", utf8_payload(header[2:])
    if re.fullmatch(rb"A[1-9][0-9]{0,5}", header) is None:
        raise ValueError("invalid status line or unexpected data after status")
    length = int(header[1:])
    body = remainder[:length]
    if len(body) != length or not body.endswith(b"\n") or remainder[length:] != b"C\n":
        raise ValueError("invalid success byte count or terminating C line")
    return "A", utf8_payload(body)


def utf8_payload(body):
    try:
        body.decode("utf-8")
    except UnicodeDecodeError:
        raise ValueError("invalid UTF-8 payload") from None
    return body


def normalise(body, attribute_padding, auth_placeholder):
    result = []
    for line in body.split(b"\n"):
        if auth_placeholder and AUTH_PLACEHOLDER.fullmatch(line):
            continue
        if attribute_padding:
            line = ATTRIBUTE_PADDING.sub(rb"\1 ", line)
        result.append(line)
    return b"\n".join(result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", type=endpoint, required=True, help="pinned upstream HOST:PORT")
    parser.add_argument("--candidate", type=endpoint, required=True, help="Rust daemon HOST:PORT")
    parser.add_argument("--corpus", type=Path, help="ASCII STATUS<TAB>!mquery file")
    parser.add_argument("--timeout", type=timeout_seconds, default=5.0, help="seconds per endpoint and case")
    parser.add_argument(
        "--normalize-attribute-padding",
        action="store_true",
        help="compare equivalent attribute separator padding inside success payloads",
    )
    parser.add_argument(
        "--allow-filtered-auth-omission",
        action="store_true",
        help="ignore known upstream password-redaction placeholders inside success payloads",
    )
    args = parser.parse_args()
    try:
        cases = read_cases(args.corpus) if args.corpus else DEFAULT_CASES
    except (OSError, ValueError) as error:
        parser.error(str(error))

    mismatches = 0
    for expected, query in cases:
        results = []
        for role, address in (("reference", args.reference), ("candidate", args.candidate)):
            try:
                status, body = frame(exchange(address, query, args.timeout))
            except (OSError, ValueError) as error:
                print(f"{query!r}: {role} failed: {error}", file=sys.stderr)
                continue
            if status != expected:
                print(f"{query!r}: {role} returned {status}, expected {expected}", file=sys.stderr)
                continue
            if status == "A":
                body = normalise(body, args.normalize_attribute_padding, args.allow_filtered_auth_omission)
            results.append(body)

        if len(results) != 2:
            mismatches += 1
        elif results[0] != results[1]:
            mismatches += 1
            print(
                f"{query!r}: response content differs "
                f"(reference {len(results[0])} bytes, candidate {len(results[1])} bytes)",
                file=sys.stderr,
            )

    print(f"{len(cases) - mismatches}/{len(cases)} WHOIS cases matched")
    return 1 if mismatches else 0


if __name__ == "__main__":
    sys.exit(main())
