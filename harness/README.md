# DDIL emulation harness

Runs an MQDB broker, three c2-fusion nodes and a router as containers on two Docker bridges, and demonstrates the nodes discovering each other through MQDB, converging over peer-to-peer QUIC, and reconverging after a network partition with a write injected during the outage. Presence is maintained by a heartbeat-renewed TTL lease, so a partitioned node's registration expires and it drops out of discovery, then returns when it reconnects and renews. This is the experimentation and simulation environment the challenge's essential outcomes call for, standing in for physical RF.

## Topology

The broker, relay and hq share one network (`c2net`, 10.231.0.0/24). Edge sits alone on a second network (`c2edge`, 10.231.1.0/24) and reaches everything else through a router container attached to both, with static routes on each side. Every bearer condition is applied on the router: `netem` on both of its interfaces shapes the edge link in both directions, and taking its edge-side interface down partitions edge. Addresses are static because Docker's name resolution does not cross networks, so the nodes reach the broker at 10.231.0.2.

## What it shows

`./run.sh` builds the images and drives the full sequence end to end:

1. Bring up an MQDB broker and three nodes (edge, relay, hq). Each node registers with the broker and discovers the others through it; each ingests one message in a wire format (edge in MTF-XML, relay and hq in `c2-json`, the canonical model as plain JSON).
2. Converge: the nodes connect peer-to-peer over QUIC and reach the same 3-message picture.
3. Degrade: apply a `tc`/`netem` rate and delay profile to the edge link, in both directions; the picture holds.
4. Partition: take the router's edge-side interface down (edge loses both the broker and its peers). While isolated it can no longer renew its presence lease, so its `$DB/peers` record expires while relay and hq keep renewing theirs.
5. Inject: drop a new contact into edge's inbox while it is isolated. Edge ingests it locally (count 4); HQ correctly still shows 3.
6. Heal and reconverge: bring the interface back up; edge renews its lease, re-discovers through MQDB, and all three nodes reach the same 4-message picture.

Last run (2026-10-11, through the router), `run.sh`:

```
c2-edge converged to 3
c2-relay converged to 3
c2-hq converged to 3
edge ingested C-2 locally (count=4) while isolated
confirmed: hq still count=3, C-2 not yet propagated
c2-edge reconverged to 4
c2-relay reconverged to 4
c2-hq reconverged to 4
DEMO PASSED: MQDB discovery, converge, degrade, partition, inject, reconverge
```

`evidence.sh` captures the same sequence with the MQDB peer registry at each phase. With the demo lease of 6 seconds, the registry during the partition shows edge's lease lapsed while the connected nodes stay fresh (a peer is live only while `_expires_at > now`):

```
now=1790874816
edge   _expires_at=1790874793  (lapsed; _version frozen at 2, no renewals since it was isolated)
relay  _expires_at=1790874821  (fresh; _version 16, still renewing)
hq     _expires_at=1790874821  (fresh; _version 16, still renewing)
```

On reconnect edge renews its lease (its `_expires_at` and `_version` advance again) and all three return to the shared 4-message picture.

## Discovery via MQDB, data plane over QUIC

Each node uses `mqp2p::Peer` to register with the MQDB broker and discover peers over its `$DB/peers` topics, and `stitch_p2p::Swarm` dials the discovered peers and bridges each into the sync session. The broker carries discovery and signaling only; the message exchange and convergence are peer-to-peer over QUIC, so the broker is not in the data path and nodes keep converging through a broker outage. STUN is disabled (`without_stun`); on a single-host bridge the offer's host candidate connects directly, so no hole-punching is needed (hole-punching is what fails on virtualized/overlay networks). A fully broker-less direct-dial mode also exists (`QuicEndpoint::connect`), covered by the `quic_exchange` integration test.

## Presence lease

Each registration carries an `_expires_at` lease (MQDB's TTL field, in unix seconds). A node renews it on a heartbeat at a third of the lease, and `list_peers` filters out any peer whose lease has lapsed, so discovery reflects liveness rather than leaving a node that has gone away as a stale `online` entry. A node isolated past its lease stops renewing and drops out of discovery; on reconnect it renews (or, if the record was already swept, re-creates it under the same id) and reappears. The nodes here use a 6-second demo lease so the lapse is visible within the partition window (`C2_LEASE_SECS` changes it); the library default is 15 seconds. Client-side filtering is load-bearing because MQDB's own TTL sweep is coarser (about 60 seconds), and it compares each reader's clock to the writer's stamp, so nodes are assumed to be roughly clock-synced.

## Broker image

The upstream MQDB image is `FROM scratch` (binary only, no writable filesystem), and this feature build requires an auth method. `mqdb.Dockerfile` repackages the mqdb binary onto Alpine and bakes in a POC-only password (`c2` / `c2pass`); the broker runs with `--passwd` and `--admin-users c2` and no ACL file (an ACL denied the broker's own internal `$DB/#` handler). Nodes authenticate with those credentials.

## Bearer shaping

- Partition and rejoin (denied/intermittent) take the router's edge-side interface down and up.
- Bandwidth and delay (degraded/low-bandwidth) use `tc`/`netem` `rate` and `delay` on both router interfaces; `run.sh` applies a UHF profile the picture holds under.
- Loss uses `netem loss` on both router interfaces in `loss.sh`, so packets to and from edge, including QUIC acknowledgements, are dropped. netem drops whole GSO super-buffers, which clusters QUIC loss (the artifact behind the withdrawn MQTT-over-QUIC paper), so the nodes run with UDP segmentation offload disabled (`C2_QUIC_SEGMENTATION_OFFLOAD=false`, which sets quinn's `enable_segmentation_offload(false)`). Each row reports the loss netem actually applied in each direction and the average bytes per packet; an average at or below the 1500-byte MTU confirms netem dropped single datagrams.

## Loss sweep

`./loss.sh` applies random loss to the edge link at 0, 1, 5, 10 and 20 percent, three trials each. Each trial injects a new contact into edge and times it from edge's ingest log line to the first hq log line naming it, using Docker's log timestamps; after a 30-second window it reads netem's counters on both router interfaces. `from_edge` is the loss on traffic leaving edge, `to_edge` the loss on traffic toward it, and `packets` the total offered in both directions. The nodes publish their picture every 200 ms in this run. `LOSS_RATES`, `LOSS_TRIALS` and `LOSS_WINDOW` override the rates, trials per rate and window; `LOSS_PROFILE` adds netem parameters applied together with the loss, so the same sweep runs under a bearer's rate and delay (for example `LOSS_PROFILE="rate 64kbit delay 40ms 15ms" LOSS_RATES="0 3" ./loss.sh`).

Last run (2026-10-11), `loss.sh`:

```
configured trial  from_edge    to_edge      packets    avg_bytes_pkt  convergence_s
0%         1      0.0%         0.0%         2132       142            0.13
0%         2      0.0%         0.0%         2140       143            0.11
0%         3      0.0%         0.0%         2106       143            0.09
1%         1      0.9%         0.8%         2184       142            0.01
1%         2      0.8%         0.8%         2115       143            0.17
1%         3      1.0%         0.9%         2188       142            0.06
5%         1      4.3%         5.1%         2451       138            0.00
5%         2      5.8%         5.3%         2487       136            0.09
5%         3      4.2%         5.3%         2448       137            0.03
10%        1      9.7%         10.5%        2826       131            0.01
10%        2      9.3%         10.4%        2837       130            0.10
10%        3      9.2%         10.3%        2789       131            0.05
20%        1      20.3%        18.6%        3667       122            0.15
20%        2      20.1%        20.0%        3612       124            0.11
20%        3      19.6%        19.7%        3852       122            0.04
```

Every contact reached hq within 0.17 seconds up to 20 percent loss in both directions. The packets offered in each window grow with loss (about 2130 at 0 percent, about 3700 at 20 percent) as QUIC retransmits. Convergence is the gap between two log lines with millisecond timestamps, so values near zero mean the contact arrived within milliseconds.

Under the bearer profiles of `netem-profiles.sh` (placeholder values, not measured bearer figures), run with `LOSS_PROFILE` and each profile's loss (2026-10-11, three trials per rate, shaping in both directions):

```
profile  netem                              loss  from_edge        to_edge          avg_bytes_pkt  convergence_s
uhf      rate 64kbit delay 40ms 15ms        0%    0.0 0.0 0.0%     0.0 0.0 0.0%     141-143        0.10 0.28 0.30
uhf      rate 64kbit delay 40ms 15ms        3%    3.0 2.9 3.0%     3.1 3.1 2.6%     143-144        0.20 0.33 0.37
satcom   rate 256kbit delay 300ms 40ms      0%    0.0 0.0 0.0%     0.0 0.0 0.0%     144-153        0.40 0.36 0.52
satcom   rate 256kbit delay 300ms 40ms      1%    0.8 0.7 0.8%     1.8 1.0 1.1%     144-147        0.37 0.46 0.48
```

### HF

At the default intervals the HF profile (rate 2400 bit/s, delay 250 ms ± 100 ms) does not converge within 120 seconds even without loss: with no messages flowing the edge already sends about 55 kbit/s (measured before the router was added, on edge's egress alone), more than twenty times the profile's rate, so the queue saturates. The likely sources are the 200 ms sync pull, the 200 ms discovery poll at the time, presence lease renewals and broker signalling; the traffic has not been broken down by source.

`C2_PULL_MS` sets the sync pull interval (default 200 ms), `C2_DISCOVERY_MS` how often a node asks the broker for peers (default 2000 ms) and `C2_LEASE_SECS` the presence lease (default 6 s, renewed at a third of it). The HF profile with the pull and discovery intervals at 30000 ms, the cadence the LoRa bearer uses, and the lease at 6 s and at 90 s (2026-10-11, through the router):

```
lease  loss  from_edge           to_edge             packets      avg_bytes_pkt  convergence_s
6 s    0%    0.0 0.0 0.0%        0.0 0.0 0.0%        113 70 77    119-139        2.19 1.61 2.22
6 s    15%   17.2 14.6 11.5%     20.0 11.4 10.0%     49 76 46     121-170        1.75 4.34 1.64
90 s   0%    0.0 0.0 0.0%        0.0 0.0 0.0%        66 68 53     88-138         2.91 1.85 2.95
90 s   15%   29.2 16.2 10.0%     11.8 7.1 0.0%       41 65 33     126-166        1.79 2.98 4.54
```

Every contact converged within 4.6 seconds, with and without loss. A new write is pushed to connected peers immediately, so the pull interval governs catch-up after a partition rather than delivery latency. Without loss the 90-second lease lowers the traffic on the link from 70 to 113 packets per 30 seconds to 53 to 68; the rest is likely broker keepalives and the remaining sync exchanges, not yet broken down. With so few packets per window, the measured loss scatters widely around the configured 15 percent.

## Monitoring

`./monitoring.sh` adds the OpenTelemetry Collector, Prometheus and Grafana (the `monitoring` Compose profile), turns on each node's OTLP exporter, and runs a partition and recovery, printing from Prometheus at each stage. The collector sits on both networks, so edge's telemetry does not cross the router and a partitioned edge stays visible. Run on 2026-10-12 with a 5 s export interval:

| Stage | Live peers (edge / hq / relay) | Distinct pictures | Replication lag |
|---|---|---|---|
| Converged | 2 / 2 / 2 | 1 | 0 for every node and origin |
| Edge partitioned, C-2 ingested at edge | 0 / 1 / 1 | 2 | hq and relay 1 write behind on origin edge |
| Healed | 2 / 2 / 2 | 1 | 0 for every node and origin |

The metrics, the queries behind these columns and the ingest paths are in [../docs/monitoring.md](../docs/monitoring.md).

## Event stream

`./events.sh` turns on the event stream at edge and hq (published on 127.0.0.1:7601 and 127.0.0.1:7600), attaches the reference consumer to each, partitions edge, injects C-2 at edge, heals, and checks each consumer's picture digest against its node's `PICTURE` digest. Run on 2026-10-13:

```
--- consumer on hq
HELLO node=hq schema=1
READY node=hq count=3
CONSUMER node=hq count=3 digest=cf369d04 labelled=0 types=ContactReport:1,Rfi:1,Sitrep:1
CHANGE node=hq seq=5 op=insert id=C-2 origin=remote type=ContactReport at=2026-10-13T19:55:25.049Z
CONSUMER node=hq count=4 digest=fb17f9a0 labelled=0 types=ContactReport:2,Rfi:1,Sitrep:1
hq: consumer picture digest fb17f9a0 matches the node's PICTURE digest
--- consumer on edge
HELLO node=edge schema=1
READY node=edge count=3
CONSUMER node=edge count=3 digest=cf369d04 labelled=0 types=ContactReport:1,Rfi:1,Sitrep:1
CHANGE node=edge seq=5 op=insert id=C-2 origin=local type=ContactReport at=2026-10-13T19:55:14.988Z
CONSUMER node=edge count=4 digest=fb17f9a0 labelled=0 types=ContactReport:2,Rfi:1,Sitrep:1
edge: consumer picture digest fb17f9a0 matches the node's PICTURE digest
edge consumer saw C-2 as a local insert during the partition
hq consumer saw C-2 as a remote insert after the heal
EVENTS PASSED: snapshot, live local and remote changes, consumer picture equals node picture
```

Edge's consumer saw C-2 at ingest during the partition, hq's about 10 seconds later, after the heal.

The line format is in [../docs/events.md](../docs/events.md).

## Run it

```
./run.sh
./loss.sh
./evidence.sh
./monitoring.sh
./events.sh
```

Requires Docker. `run.sh` and `loss.sh` build the MQDB broker image (from the sibling MQDB repo, skipped if already present), the Alpine-repackaged broker image, the router image, and the `c2-node` image (whose sibling crates are fetched as git dependencies), bring the stack up, run their sequence, and tear everything down; `evidence.sh` reuses the images they built.
