# DDIL emulation harness

Runs an MQDB broker and three c2-fusion nodes as containers on a Docker bridge, and demonstrates the nodes discovering each other through MQDB, converging over peer-to-peer QUIC, and reconverging after a network partition with a write injected during the outage. This is the experimentation and simulation environment the challenge's essential outcomes call for, standing in for physical RF.

## What it shows

`./run.sh` builds the images and drives the full sequence end to end:

1. Bring up an MQDB broker and three nodes (edge, relay, hq). Each node registers with the broker and discovers the others through it; each ingests one message in a wire format (edge in MTF-XML, relay and hq in NIEM-JSON).
2. Converge: the nodes connect peer-to-peer over QUIC and reach the same 3-message picture.
3. Degrade: apply a `tc`/`netem` rate and delay profile to the edge uplink; the picture holds.
4. Partition: disconnect edge from the network (it loses both the broker and its peers).
5. Inject: drop a new contact into edge's inbox while it is isolated. Edge ingests it locally (count 4); HQ correctly still shows 3.
6. Heal and reconverge: reconnect edge; it re-discovers through MQDB and all three nodes reach the same 4-message picture.

Last run (2026-09-30):

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

## Discovery via MQDB, data plane over QUIC

Each node uses `mqp2p::Peer` to register with the MQDB broker and discover peers over its `$DB/peers` topics, and `stitch_p2p::Swarm` dials the discovered peers and bridges each into the sync session. The broker carries discovery and signaling only; the message exchange and convergence are peer-to-peer over QUIC, so the broker is not in the data path and nodes keep converging through a broker outage. STUN is disabled (`without_stun`); on a single-host bridge the offer's host candidate connects directly, so no hole-punching is needed (hole-punching is what fails on virtualized/overlay networks). A fully broker-less direct-dial mode also exists (`QuicEndpoint::connect`), covered by the `quic_exchange` integration test.

## Broker image

The upstream MQDB image is `FROM scratch` (binary only, no writable filesystem), and this feature build requires an auth method. `mqdb.Dockerfile` repackages the mqdb binary onto Alpine and bakes in a POC-only password (`c2` / `c2pass`); the broker runs with `--passwd` and `--admin-users c2` and no ACL file (an ACL denied the broker's own internal `$DB/#` handler). Nodes authenticate with those credentials.

## Bearer shaping

- Partition and rejoin (denied/intermittent) use `docker network disconnect`/`connect`, no netem.
- Bandwidth and delay (degraded/low-bandwidth) use `tc`/`netem` `rate` and `delay`; the runtime image installs `iproute2`, so `run.sh` applies a UHF profile the picture holds under.
- Loss is deliberately not scripted: netem drops whole GSO super-buffers and clusters QUIC loss (the artifact behind the withdrawn MQTT-over-QUIC paper). Trust netem loss only with UDP segmentation offload disabled.

## Run it

```
./run.sh
```

Requires Docker. The script builds the MQDB broker image (from the sibling MQDB repo, skipped if already present), the Alpine-repackaged broker image, and the `c2-node` image (whose sibling crates are fetched as git dependencies), brings the stack up, runs the demo, and tears everything down.
