# Monitoring exporter

`c2-node` exports operational telemetry over OpenTelemetry (OTLP), so the exchange can be watched from existing monitoring tooling. The exporter maps what the node already knows to metrics: the store's mutation events (the reactive event stream), the presence leases in the discovery broker, the replication cursors, and the shared picture itself. It adds no new protocol traffic between nodes; the only new traffic is the export to the collector.

## Interface

The exporter is off unless an OTLP endpoint is set, and is configured with the standard OpenTelemetry environment variables, so any OTLP-compatible collector can receive it without c2-specific setup.

| Variable | Meaning | Default |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Collector base URL, OTLP over HTTP with protobuf (for example `http://collector:4318`; metrics go to `/v1/metrics`). Telemetry is off when unset or empty. | unset |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Full metrics URL, overriding the base URL. | unset |
| `OTEL_METRIC_EXPORT_INTERVAL` | Milliseconds between exports, and between samples of the gauges below. | `60000` |
| `OTEL_RESOURCE_ATTRIBUTES` | Extra resource attributes, `key=value,...`. | none |

Every metric carries the resource attributes `service.name=c2-node` and `service.instance.id=<C2_NAME>`. In Prometheus these become `job="c2-node"` and `instance="<node name>"`.

Transport is plain HTTP, so a node needs no TLS stack for telemetry; put the collector on a trusted management network or terminate TLS at the collector.

## Metrics

OpenTelemetry names are shown first; the Prometheus name, after the collector's standard conversion, follows in brackets.

### Node connectivity (from the presence lease)

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `c2.peers.live` (`c2_peers_live`) | gauge | | Other nodes currently holding a live presence lease in the discovery broker. |
| `c2.peer.lease_remaining` (`c2_peer_lease_remaining_seconds`) | gauge, seconds | `peer` | Seconds left on each peer's lease as this node sees it. A peer that has dropped out of discovery is reported as 0, so a lapse shows as a fall to zero rather than a gap. |
| `c2.broker.reachable` (`c2_broker_reachable`) | gauge | | 1 when this node's last presence lookup succeeded, 0 when the broker could not be reached or the node has not yet joined. |

### Replication lag

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `c2.replication.cursor` (`c2_replication_cursor`) | gauge | `origin` | Highest write sequence this node has delivered from each origin node. `origin` is the origin's node name when known, otherwise the first four bytes of its id in hex. |

Lag is derived at the monitoring platform, by comparing cursors across nodes: for each origin, the highest cursor any node reports minus this node's cursor is the number of that origin's writes this node has not yet received. In PromQL:

```
max by (origin) (c2_replication_cursor) - on (origin) group_right () c2_replication_cursor
```

This is lag in writes, not seconds. Sequences are per origin and only grow, so the difference is exact. A node that cannot export (for example one cut off from the collector) stops updating its own series, so read lag together with `c2_broker_reachable` and export freshness.

### Convergence state

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `c2.picture.records` (`c2_picture_records`) | gauge | | Messages in this node's common operational picture. |
| `c2.picture.unreadable` (`c2_picture_unreadable`) | gauge | | Records in the store that are not readable C2 messages. They are left out of the picture, its record count and its digest, and listed on the `PICTURE` line as `unreadable=`. Non-zero needs attention: a node is writing records the others cannot read. |
| `c2.picture.digest` (`c2_picture_digest`) | gauge | | First 32 bits of the SHA-256 of the node's picture, the same digest the node logs on its `PICTURE` line. Equal values mean equal pictures. |

The system is converged when every node reports the same digest:

```
count(count_values("digest", c2_picture_digest))
```

returns 1 when converged and the number of distinct pictures otherwise.

### Message counts

| Metric | Type | Attributes | Meaning |
|---|---|---|---|
| `c2.mutations` (`c2_mutations_total`) | counter | `origin` (`local` or `remote`), `op` (`insert`, `update`, `delete`), `message_type` (for example `ContactReport`, `unknown` for deletes), `labelled` (`true` when the message carries a confidentiality label) | Changes to the visible picture. A local insert is a message ingested at this node; a remote one arrived by replication. |
| `c2.telemetry.events_missed` (`c2_telemetry_events_missed_total`) | counter | | Mutation events the exporter fell behind on and could not count. Non-zero means `c2_mutations_total` undercounts. |

The exporter subscribes to the event stream before the node ingests anything, so messages loaded at start-up are counted. A new combination of attributes starts a new series at 1, which a rate over that series cannot show, so the example dashboard plots the cumulative count summed by node and origin.

Message counts come from the store's mutation event stream, so they count changes that won at the conflict-resolution step. A replicated write that loses to a newer one does not change the picture and is not counted.

## Ingest paths

### Prometheus and Grafana

The harness runs the OpenTelemetry Collector (contrib distribution) with an OTLP receiver and its Prometheus exporter, which Prometheus scrapes; Grafana reads Prometheus. The configuration is in `harness/monitoring/`:

- `otel-collector.yaml`: OTLP/HTTP receiver on port 4318, Prometheus exporter on 8889.
- `prometheus.yml`: scrapes the collector every 5 s.
- `grafana/`: a provisioned Prometheus data source and one example dashboard, `c2-overview.json`, built only from the queries on this page.

Prometheus 3 can also receive OTLP directly (`--web.enable-otlp-receiver`, path `/api/v1/otlp/v1/metrics`), which removes the collector for a Prometheus-only deployment.

### Splunk

Splunk ingests OpenTelemetry through the HTTP Event Collector (HEC), using the Splunk distribution of the OpenTelemetry Collector or the contrib collector's `splunk_hec` exporter. Adding Splunk to the harness collector is an exporter entry and a pipeline change:

```yaml
exporters:
  splunk_hec:
    token: "${env:SPLUNK_HEC_TOKEN}"
    endpoint: "https://splunk.example:8088/services/collector"
    source: c2-fusion
    sourcetype: otel
    index: metrics

service:
  pipelines:
    metrics:
      exporters: [prometheus, splunk_hec]
```

This path has not been run against a Splunk instance.

## Running it

```
harness/monitoring.sh
```

builds the images, starts the router, broker and three nodes plus the collector, Prometheus and Grafana (the `monitoring` Compose profile), and then partitions edge, injects a contact while it is cut off, and heals the partition, printing peer counts, distinct pictures and replication lag from Prometheus at each stage. `KEEP=1` leaves the stack running afterwards; Grafana is at http://127.0.0.1:3000 and Prometheus at http://127.0.0.1:9090, both bound to localhost only. `HOLD` sets the seconds to wait at each stage (default 30) and `C2_OTLP_INTERVAL_MS` the export interval (default 5000).

In the harness the collector sits on both the core and the edge networks, so telemetry from edge reaches it directly rather than through the router where the bearer conditions are applied. That keeps a partitioned node visible, which is what the demonstration needs, and keeps telemetry out of the loss and convergence measurements. In a deployment, telemetry shares the bearer with the exchange unless there is a separate management link, and on a narrow bearer such as HF the export interval has to be sized to the link: the export size was not measured here.
