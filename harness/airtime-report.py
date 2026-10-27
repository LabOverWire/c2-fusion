import collections
import subprocess
import sys

FIELDS = [
    "frame.time_relative",
    "frame.len",
    "ip.src",
    "ip.dst",
    "tcp.srcport",
    "tcp.dstport",
    "tcp.len",
    "tcp.flags.syn",
    "tcp.flags.fin",
    "tcp.flags.reset",
    "tcp.analysis.retransmission",
    "udp.srcport",
    "mqtt.msgtype",
    "mqtt.topic",
]

MQTT_TYPES = {
    "1": "CONNECT",
    "2": "CONNACK",
    "3": "PUBLISH",
    "4": "PUBACK",
    "8": "SUBSCRIBE",
    "9": "SUBACK",
    "10": "UNSUBSCRIBE",
    "11": "UNSUBACK",
    "12": "PINGREQ",
    "13": "PINGRESP",
    "14": "DISCONNECT",
}

NAMES = {"10.231.1.10": "edge", "10.231.0.2": "mqdb", "10.231.0.11": "relay", "10.231.0.12": "hq"}


def topic_kind(topic):
    parts = topic.split("/")
    if topic.startswith("$DB/peers/"):
        return "$DB/peers/" + parts[-1]
    if topic.startswith("$DB/"):
        return "$DB/" + parts[1] + "/" + parts[-1]
    if topic.startswith("p2p/"):
        return "p2p/<peer>/" + parts[-1]
    if topic.startswith("mqp2p/responses/"):
        return "mqp2p/responses/<request>"
    return parts[0]


def flag(row, field):
    return row[field] in ("1", "True")


def category(row):
    if row["udp.srcport"]:
        peer = row["ip.dst"] if row["ip.src"] == "10.231.1.10" else row["ip.src"]
        return "QUIC sync edge-" + NAMES.get(peer, peer)
    if not row["tcp.srcport"]:
        return "ARP/ICMPv6"
    if row["tcp.analysis.retransmission"]:
        return "TCP retransmission"
    if flag(row, "tcp.flags.syn") or flag(row, "tcp.flags.fin") or flag(row, "tcp.flags.reset"):
        if row["tcp.len"] in ("", "0"):
            return "TCP connect/close/reset"
    if row["tcp.len"] in ("", "0"):
        return "TCP ack"
    types = row["mqtt.msgtype"].split(",") if row["mqtt.msgtype"] else []
    if not types:
        return "MQTT unparsed"
    first = MQTT_TYPES.get(types[0], "type " + types[0])
    if first == "PUBLISH" and row["mqtt.topic"]:
        return "MQTT PUBLISH " + topic_kind(row["mqtt.topic"].split(",")[0])
    if first in ("SUBSCRIBE", "UNSUBSCRIBE") and row["mqtt.topic"]:
        return "MQTT " + first + " " + topic_kind(row["mqtt.topic"].split(",")[0])
    return "MQTT " + first


def read(pcap):
    out = subprocess.run(
        ["tshark", "-o", "mqtt.default_version:MQTT v5.0", "-r", pcap, "-T", "fields", "-E", "separator=\t", *[a for f in FIELDS for a in ("-e", f)]],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [dict(zip(FIELDS, line.split("\t"))) for line in out.splitlines()]


def main():
    pcap, window = sys.argv[1], float(sys.argv[2])
    rows = read(pcap)
    totals = collections.defaultdict(lambda: [0, 0, 0, 0])
    connections = set()
    for row in rows:
        outbound = 0 if row["ip.src"] == "10.231.1.10" else 2
        cat = category(row)
        totals[cat][outbound] += 1
        totals[cat][outbound + 1] += int(row["frame.len"])
        if flag(row, "tcp.flags.syn") and row["tcp.dstport"] == "1883":
            connections.add(row["tcp.srcport"])
    all_bytes = sum(t[1] + t[3] for t in totals.values())
    from_edge = sum(t[1] for t in totals.values())
    to_edge = sum(t[3] for t in totals.values())
    print(f"window {window:.0f} s, {len(rows)} packets, {all_bytes} bytes (Ethernet frames), from edge {from_edge * 8 / window:.0f} bit/s, to edge {to_edge * 8 / window:.0f} bit/s, {len(connections)} MQTT connections opened")
    print(f"{'category':<52} {'from_edge':>16} {'to_edge':>16} {'share':>6} {'bit/s':>6}")
    for cat, (op, ob, ip, ib) in sorted(totals.items(), key=lambda kv: -(kv[1][1] + kv[1][3])):
        share = (ob + ib) / all_bytes * 100 if all_bytes else 0
        print(f"{cat:<52} {op:>5} pkt {ob:>6} B {ip:>5} pkt {ib:>6} B {share:>5.1f}% {(ob + ib) * 8 / window:>6.0f}")


main()
