# DDIL emulation harness

Runs the c2-fusion node as three containers on a Docker bridge and demonstrates the shared C2 picture converging over real QUIC, then reconverging after a network partition with a write injected during the outage. This is the experimentation and simulation environment the challenge's essential outcomes call for, standing in for physical RF.

## What it shows

`./run.sh` builds the node image and drives the full sequence end to end:

1. Bring up a ring: edge -> relay -> hq -> edge. Each node ingests one message in a wire format (edge in MTF-XML, relay and hq in NIEM-JSON) and replicates over QUIC.
2. Converge: all three nodes reach the same 3-message picture.
3. Partition: disconnect edge from the network.
4. Inject: drop a new contact into edge's inbox while it is isolated. Edge ingests it locally (count 4); HQ correctly still shows 3.
5. Heal and reconverge: reconnect edge; all three nodes reach the same 4-message picture.

Last run (2026-09-29):

```
c2-edge converged to 3
c2-relay converged to 3
c2-hq converged to 3
edge ingested C-2 locally (count=4) while isolated
confirmed: hq still count=3, C-2 not yet propagated
c2-edge reconverged to 4
c2-relay reconverged to 4
c2-hq reconverged to 4
DEMO PASSED: converge, degrade, partition, inject, reconverge
```

## Why direct QUIC dial

The node connects with `QuicEndpoint::connect` to a known address, fingerprint-pinned, with no STUN and no UDP hole-punching. Hole-punching (mqp2p's `Peer`/`Swarm` path) fails inside virtualized and container networks; direct dial does not, because containers on one bridge have known addresses and no NAT between them. Nodes exchange fingerprints and advertise addresses through files on a shared volume, so no signaling broker is needed.

## Transport and bearer shaping

- Partition and rejoin (denied/intermittent) use `docker network disconnect`/`connect` and need no netem.
- Bandwidth and delay (degraded/low-bandwidth) use `tc`/`netem` `rate` and `delay`. The `run.sh` step applies a profile only if the image carries `iproute2`; the slim runtime image built here does not, and the step is skipped. To exercise shaping, build a node image with `iproute2` present (needs network access at image-build time).
- Loss is deliberately not scripted: netem drops whole GSO super-buffers and clusters QUIC loss (the artifact behind the withdrawn MQTT-over-QUIC paper). Trust netem loss only with UDP segmentation offload disabled.

## Run it

```
./run.sh
```

Requires Docker. The script builds `c2-node:poc` from a temporary staging context (the two sibling stitch-rs crates plus c2-fusion, no target dirs), brings the ring up, runs the demo, and tears everything down.
