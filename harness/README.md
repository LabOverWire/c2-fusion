# DDIL emulation harness

Runs an MQDB broker and three c2-fusion nodes as containers on a Docker bridge, and demonstrates the nodes discovering each other through MQDB, converging over peer-to-peer QUIC, and reconverging after a network partition with a write injected during the outage. Presence is maintained by a heartbeat-renewed TTL lease, so a partitioned node's registration expires and it drops out of discovery, then returns when it reconnects and renews. This is the experimentation and simulation environment the challenge's essential outcomes call for, standing in for physical RF.

## What it shows

`./run.sh` builds the images and drives the full sequence end to end:

1. Bring up an MQDB broker and three nodes (edge, relay, hq). Each node registers with the broker and discovers the others through it; each ingests one message in a wire format (edge in MTF-XML, relay and hq in `c2-json`, the canonical model as plain JSON).
2. Converge: the nodes connect peer-to-peer over QUIC and reach the same 3-message picture.
3. Degrade: apply a `tc`/`netem` rate and delay profile to the edge uplink; the picture holds.
4. Partition: disconnect edge from the network (it loses both the broker and its peers). While isolated it can no longer renew its presence lease, so its `$DB/peers` record expires while relay and hq keep renewing theirs.
5. Inject: drop a new contact into edge's inbox while it is isolated. Edge ingests it locally (count 4); HQ correctly still shows 3.
6. Heal and reconverge: reconnect edge; it renews its lease, re-discovers through MQDB, and all three nodes reach the same 4-message picture.

Last run (2026-10-01), `run.sh`:

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

Each registration carries an `_expires_at` lease (MQDB's TTL field, in unix seconds). A node renews it on a heartbeat at a third of the lease, and `list_peers` filters out any peer whose lease has lapsed, so discovery reflects liveness rather than leaving a node that has gone away as a stale `online` entry. A node isolated past its lease stops renewing and drops out of discovery; on reconnect it renews (or, if the record was already swept, re-creates it under the same id) and reappears. The nodes here use a 6-second demo lease so the lapse is visible within the partition window; the library default is 15 seconds. Client-side filtering is load-bearing because MQDB's own TTL sweep is coarser (about 60 seconds), and it compares each reader's clock to the writer's stamp, so nodes are assumed to be roughly clock-synced.

## Broker image

The upstream MQDB image is `FROM scratch` (binary only, no writable filesystem), and this feature build requires an auth method. `mqdb.Dockerfile` repackages the mqdb binary onto Alpine and bakes in a POC-only password (`c2` / `c2pass`); the broker runs with `--passwd` and `--admin-users c2` and no ACL file (an ACL denied the broker's own internal `$DB/#` handler). Nodes authenticate with those credentials.

## Bearer shaping

- Partition and rejoin (denied/intermittent) use `docker network disconnect`/`connect`, no netem.
- Bandwidth and delay (degraded/low-bandwidth) use `tc`/`netem` `rate` and `delay`; the runtime image installs `iproute2`, so `run.sh` applies a UHF profile the picture holds under.
- Loss uses `netem loss` in `loss.sh`. netem drops whole GSO super-buffers, which clusters QUIC loss (the artifact behind the withdrawn MQTT-over-QUIC paper), so the nodes run with UDP segmentation offload disabled (`C2_QUIC_SEGMENTATION_OFFLOAD=false`, which sets quinn's `enable_segmentation_offload(false)`). Each row reports the loss netem actually applied and the average bytes per packet it saw; an average at or below the 1500-byte MTU confirms netem dropped single datagrams.

## Loss sweep

`./loss.sh` applies random loss to the edge uplink at 0, 1, 5, 10 and 20 percent, three trials each. Each trial injects a new contact into edge and times it from edge's ingest log line to the first hq log line naming it, using Docker's log timestamps; after a 30-second window it reads netem's counters. The nodes publish their picture every 200 ms in this run. Loss is applied to edge's egress only, so packets toward edge, including QUIC acknowledgements, are not dropped. `LOSS_RATES`, `LOSS_TRIALS` and `LOSS_WINDOW` override the rates, trials per rate and window; `LOSS_PROFILE` adds netem parameters applied together with the loss, so the same sweep runs under a bearer's rate and delay (for example `LOSS_PROFILE="rate 64kbit delay 40ms 15ms" LOSS_RATES="0 3" ./loss.sh`).

Last run (2026-10-11), `loss.sh`:

```
configured trial  measured     packets    avg_bytes_pkt  convergence_s
0%         1      0.0%         1732       120            0.18
0%         2      0.0%         1710       121            0.15
0%         3      0.0%         1716       121            0.05
1%         1      0.9%         1740       121            0.18
1%         2      0.8%         1722       121            0.13
1%         3      1.2%         1733       121            0.17
5%         1      4.0%         1732       122            0.09
5%         2      6.0%         1705       123            0.16
5%         3      4.9%         1768       122            0.12
10%        1      10.5%        1859       121            0.07
10%        2      9.5%         1847       122            0.11
10%        3      9.8%         1848       121            0.03
20%        1      19.1%        1908       122            0.11
20%        2      19.9%        1954       121            0.06
20%        3      21.4%        2059       120            0.20
```

Every contact reached hq within 0.2 seconds up to 20 percent loss. The packets offered in each window grow with loss (about 1720 at 0 percent, about 1970 at 20 percent) as QUIC retransmits. Convergence is measured to the first hq log line, so it carries the timing of the 200 ms picture log.

Under the bearer profiles of `netem-profiles.sh` (placeholder values, not measured bearer figures), run with `LOSS_PROFILE` and each profile's loss (2026-10-11, three trials per rate):

```
profile  netem                              loss  measured        avg_bytes_pkt  convergence_s
uhf      rate 64kbit delay 40ms 15ms        0%    0.0 0.0 0.0%    126-127        0.34 0.27 0.20
uhf      rate 64kbit delay 40ms 15ms        3%    3.2 2.7 2.5%    127-128        0.27 0.19 0.32
satcom   rate 256kbit delay 300ms 40ms      0%    0.0 0.0 0.0%    130-131        0.33 0.48 0.45
satcom   rate 256kbit delay 300ms 40ms      1%    1.2 1.0 1.0%    132-133        0.34 0.51 0.40
hf       rate 2400bit delay 250ms 100ms     0%    did not converge within 120 s
```

Under HF the QUIC path does not converge even without loss: with no messages flowing, the edge already sends about 55 kbit/s (about 1720 packets of about 121 bytes per 30 seconds in the loss sweep), more than twenty times the profile's rate, so the queue saturates. The likely sources are the 200 ms sync polling interval, presence lease renewals and broker signalling; the traffic has not been broken down by source.

`C2_PULL_MS` sets the sync pull interval (default 200 ms) and `C2_DISCOVERY_MS` how often a node asks the broker for peers (default 2000 ms). Rerun of the HF profile with both at 30000 ms, the cadence the LoRa bearer uses (2026-10-11):

```
profile  netem                              loss  measured           packets   avg_bytes_pkt  convergence_s
hf       rate 2400bit delay 250ms 100ms     0%    0.0 0.0 0.0%       75 60 62  102-124        2.82 6.25 1.58
hf       rate 2400bit delay 250ms 100ms     15%   17.5 26.4 20.8%    63 53 53  121-127        1.73 2.10 2.85
```

Every contact converged within 6.3 s, with and without loss. A new write is pushed to connected peers immediately, so the pull interval governs catch-up after a partition rather than delivery latency. The edge still offers about 60 packets per 30 seconds (about 2 kbit/s, most of the HF rate), likely from presence lease renewals every 2 seconds (a third of the 6-second demo lease), broker keepalives and the remaining sync exchanges. With so few packets per window, the measured loss scatters widely around the configured 15%.

## Run it

```
./run.sh
./loss.sh
```

Requires Docker. The script builds the MQDB broker image (from the sibling MQDB repo, skipped if already present), the Alpine-repackaged broker image, and the `c2-node` image (whose sibling crates are fetched as git dependencies), brings the stack up, runs the demo, and tears everything down.
