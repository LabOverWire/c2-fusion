# Event stream

`c2-node` can expose the store's reactive event stream to programs outside the node. This is the integration point for a future analytics or model layer (data fusion, message prioritisation, decision support): such a layer reads every change to the shared picture as it happens, with its metadata, and needs no access to the replication protocol, the radio bearer or any wire format. Nothing here performs fusion or prioritisation; the stream is the attachment point, and `examples/events_consumer.py` is a reference consumer.

## Interface

The stream is off unless `C2_EVENTS_LISTEN` is set to a TCP address (for example `127.0.0.1:7600`, or `0.0.0.0:7600` inside a container). Each client that connects receives newline-delimited JSON, one object per line, in UTF-8. The node writes and never reads; a client disconnects by closing the connection. Up to 8 clients can be connected at once; further connections are closed immediately.

There is no authentication or encryption on the socket. Bind it to localhost, or to a trusted management interface, so that only co-located or management-network consumers can read the picture.

## Lines

Every line has `seq`, a counter that starts at 0 for each connection and rises by 1 per line, and `kind`. A gap in `seq` would mean a lost line; the reference consumer treats one as a fatal error.

| `kind` | Fields | Meaning |
|---|---|---|
| `hello` | `node`, `schema` | First line. `node` is the node's name (`C2_NAME`); `schema` is the version of this line format, currently 1. |
| `snapshot` | `id`, `origin`, `record` | One message of the current picture, in id order. `origin` is `local` when this node wrote the winning version, `remote` when it arrived by replication. |
| `ready` | `count`, `at` | End of the snapshot: `count` snapshot lines were sent. From here on the client holds the node's picture as of `at`. |
| `change` | `op`, `id`, `origin`, `at`, `record` | A change to the picture. `op` is `insert`, `update` or `delete`; `record` is `null` for a delete. `at` is the node's time when it forwarded the change (RFC 3339, milliseconds). |
| `resync` | `missed` | The client fell behind the node's event buffer and `missed` changes were dropped. A fresh snapshot and `ready` follow; the client discards its copy and rebuilds from them. |

`record` is the message in the canonical `c2-json` form (the canonical model as plain JSON, field order as the model defines it), so a consumer never deals with MTF, NIEM, COIE text or the compact radio encoding, whatever format the message arrived in. A confidentiality label, when present, is the record's `label` field.

Example (shortened):

```
{"seq":0,"kind":"hello","node":"hq","schema":1}
{"seq":1,"kind":"snapshot","id":"C-1","origin":"remote","record":{"message_type":"ContactReport","id":"C-1",...}}
{"seq":4,"kind":"ready","count":3,"at":"2026-10-13T18:02:11.204Z"}
{"seq":5,"kind":"change","op":"insert","id":"C-2","origin":"remote","at":"2026-10-13T18:02:40.917Z","record":{"message_type":"ContactReport","id":"C-2",...}}
```

## Semantics

- **No gap between snapshot and changes.** The node subscribes a client to the event stream before it reads the snapshot. A change that lands while the snapshot is being read is therefore in both, and the client applies it twice. Because every `change` carries the whole record, applying the changes in order after the snapshot ends at the node's picture.
- **Winning changes only.** Changes come from the store's mutation events, which fire when the visible state changes, after conflict resolution. A replicated write that loses to a newer one is not sent.
- **Bounded memory.** The node never queues for a slow client beyond its fixed event buffer (1024 changes in stitch-p2p). A client that falls further behind gets `resync` and a new snapshot instead of an unbounded backlog, and it never slows the exchange itself.
- **Checking the copy.** A client can check that its copy equals the node's picture: serialise the copy as one JSON object from id to record, ids sorted, records exactly as received and no whitespace, and the first 4 bytes of its SHA-256 equal the digest on the node's `PICTURE` log line and in the `c2.picture.digest` metric ([monitoring.md](monitoring.md)). The reference consumer prints this digest.

## Writing back

A model layer that produces something new (a fused track, a priority, an alert) feeds it back the same way any other source does: as a message ingested at a node, through the inbox directory (`C2_INBOX`, in any registered codec's format) or a codec in the ingest path. It then replicates like any other message and reaches every node's stream. A field or message type for derived products is not defined yet.

## Reference consumer

`examples/events_consumer.py` (Python 3 standard library only) connects, mirrors the picture from the stream, checks `seq` continuity, prints one line per change, and prints a summary with the record count, the digest, the number of labelled records and a count per message type when the snapshot is complete and when it stops (on SIGTERM, Ctrl-C, end of stream or after `--seconds`).

```
python3 examples/events_consumer.py --host 127.0.0.1 --port 7600
```

It is where an analytics layer would start: replace the per-change print with the model's input.

## Running it

`harness/events.sh` starts the harness with the stream on at edge (published on 127.0.0.1:7601) and hq (127.0.0.1:7600), attaches the reference consumer to both, partitions edge, injects a contact at edge while it is cut off, heals the partition, and then checks that each consumer's digest equals its node's `PICTURE` digest and that edge's consumer saw the contact as a local insert and hq's as a remote one. The ports are published to the host directly, so the consumers stay connected while edge is partitioned from the other nodes.
