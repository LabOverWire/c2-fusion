# DDIL emulation harness

Emulates contested tactical-bearer conditions so the message exchange can be exercised under Denied, Degraded, Intermittent, and Low-bandwidth (DDIL) links without physical RF equipment. This is the experimentation and simulation environment referenced in the challenge's essential outcomes.

## Why containers

`tc`/`netem` is Linux-only and needs `NET_ADMIN`. The development host is macOS, so the three nodes run as Linux containers on a shared bridge network. Each container applies a bearer profile to its own interface, so a node's uplink can be shaped or dropped independently.

## Nodes

- `node-edge`: the forward node that originates a contact/spot report (the drone/ISR operator in the demo scenario).
- `node-relay`: an intermediate relay.
- `node-hq`: the command post rendering the shared C2 picture.

## Bearer profiles

`netem-profiles.sh <interface> <hf|uhf|satcom|clear>` applies one profile. The rate, delay, and loss values are placeholders. They are refined once real HF/UHF/SATCOM figures are gathered in the reading track (see `../../BuyCanadian/rd-poc-scope.md`).

| Profile | Rate | Delay | Loss |
|---|---|---|---|
| hf | 2400 bit/s | 250 ms +/- 100 ms | 15% |
| uhf | 64 kbit/s | 40 ms +/- 15 ms | 3% |
| satcom | 256 kbit/s | 300 ms +/- 40 ms | 1% |

## Usage

```
docker compose up -d
docker exec c2-node-edge bash /opt/netem-profiles.sh eth0 hf
# exercise the exchange, then simulate a denied link:
docker network disconnect harness_tactical c2-node-edge   # partition
docker network connect harness_tactical c2-node-edge      # rejoin; picture converges
docker compose down
```

## What runs on the nodes

Currently the nodes are bare Debian containers used to validate the profiles and partition behaviour. The exchange binary (codec plus stitch-p2p sync) is added here in the build phase; the convergence guarantee it relies on is machine-checked in the stitch-p2p TLA+ specs (`InvConvergence`).
