#!/usr/bin/env python3
import argparse
import hashlib
import json
import signal
import socket
import sys
import time
from collections import Counter

SCHEMA = 2


def canonical(record):
    return json.dumps(record, separators=(",", ":"), ensure_ascii=False)


class Mirror:
    def __init__(self, node):
        self.node = node
        self.picture = {}
        self.expected = 0

    def digest(self):
        body = ",".join(json.dumps(key, ensure_ascii=False) + ":" + self.picture[key] for key in sorted(self.picture))
        return hashlib.sha256(("{" + body + "}").encode()).hexdigest()[:8]

    def summary(self):
        records = [json.loads(text) for text in self.picture.values()]
        types = Counter(record["message_type"] for record in records)
        labelled = sum(1 for record in records if "label" in record)
        listed = ",".join(f"{name}:{count}" for name, count in sorted(types.items()))
        return f"CONSUMER node={self.node} count={len(self.picture)} digest={self.digest()} labelled={labelled} types={listed}"

    def apply(self, line):
        if line["seq"] != self.expected:
            sys.exit(f"CONSUMER gap: expected seq {self.expected}, got {line['seq']}")
        self.expected += 1
        kind = line["kind"]
        if kind == "hello":
            if line["schema"] != SCHEMA:
                sys.exit(f"CONSUMER unsupported schema {line['schema']}")
            self.node = line["node"]
            print(f"HELLO node={self.node} schema={line['schema']}", flush=True)
        elif kind == "snapshot":
            self.picture[line["id"]] = canonical(line["record"])
        elif kind == "ready":
            print(f"READY node={self.node} count={line['count']}", flush=True)
            print(self.summary(), flush=True)
        elif kind == "change":
            record = line["record"]
            if record is None:
                self.picture.pop(line["id"], None)
                message_type = "-"
            else:
                self.picture[line["id"]] = canonical(record)
                message_type = record["message_type"]
            print(
                f"CHANGE node={self.node} seq={line['seq']} op={line['op']} id={line['id']} "
                f"origin={line['origin']} type={message_type} at={line['at']}",
                flush=True,
            )
        elif kind == "unreadable":
            self.picture.pop(line["id"], None)
            print(f"UNREADABLE node={self.node} seq={line['seq']} id={line['id']} origin={line['origin']} error={line['error']}", flush=True)
        elif kind == "resync":
            print(f"RESYNC node={self.node} missed={line['missed']}", flush=True)
            self.picture.clear()


def connect(host, port, deadline):
    while True:
        try:
            return socket.create_connection((host, port), timeout=5)
        except OSError as error:
            if time.monotonic() > deadline:
                sys.exit(f"CONSUMER cannot connect to {host}:{port}: {error}")
            time.sleep(1)


def lines(sock, deadline):
    buffer = b""
    sock.settimeout(1)
    while time.monotonic() < deadline:
        try:
            chunk = sock.recv(65536)
        except socket.timeout:
            continue
        if not chunk:
            return
        buffer += chunk
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            yield json.loads(line)


def main():
    signal.signal(signal.SIGTERM, signal.default_int_handler)
    parser = argparse.ArgumentParser(description="Example c2-node event stream consumer")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7600)
    parser.add_argument("--seconds", type=float, default=0, help="stop after this long (0 runs until the node closes)")
    arguments = parser.parse_args()
    deadline = time.monotonic() + arguments.seconds if arguments.seconds else float("inf")
    mirror = Mirror(f"{arguments.host}:{arguments.port}")
    try:
        for line in lines(connect(arguments.host, arguments.port, deadline), deadline):
            mirror.apply(line)
    except KeyboardInterrupt:
        pass
    print(mirror.summary(), flush=True)


if __name__ == "__main__":
    main()
