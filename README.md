# c2-fusion

A proof-of-concept for turning real-time operational data into standards-compliant Command and Control (C2) messages and exchanging them across nodes over contested, low-bandwidth links, keeping every node's view convergent when links drop and rejoin.

Prototype developed for the DND IDEaS Competitive Projects challenge W7714-248676/017 (RF Data Fusion for Command and Control), Component 1a. It is research-stage (TRL 2 to 3): the enabling substrate is mature and in production, and this repository is the challenge-specific R&D built on top of it.

## Design

A canonical, format-independent C2 message model with a pluggable codec layer. Military wire formats map onto the model as codecs, so support for a new or changed standard is a new codec, not a change to the core. This directly answers the challenge's requirement for a modular, extensible design that accommodates emerging standards.

- `crates/c2-model`: the canonical model. Contact report, situation report, and request for information, each tagged with an operational domain and functional service.
- `crates/c2-codec`: the `Codec` trait, a registry, and two working codecs. `mtf-xml` (an XML Message Text Format representation, in the family of NATO APP-11 XML-MTF) and `niem-json` (a NIEM-style JSON exchange). Both round-trip the same canonical message, which is what proves the format-agnostic claim.
- `crates/c2-exchange`: the ingest and egress boundary. A wire-format message is decoded by a codec and published to the stitch-p2p store; the converged store projects back into a shared C2 picture that can be re-exported in any codec's format. Integration tests show two nodes receiving different wire formats converge to one picture, that picture re-exports in a single format, and the picture reconverges after a link partition and rejoin.
- `crates/c2-node`: the node binary. It registers with an MQDB broker under a heartbeat-renewed presence lease, discovers peers through it (`$DB/peers`), and lets `stitch_p2p::Swarm` bridge each discovered peer into the sync session over QUIC.
- `harness/`: the DDIL emulation environment. An MQDB broker container plus three node containers on a shared network. The demo shows nodes discovering each other through MQDB and converging, then partition and rejoin (denied/intermittent) with a write injected during the outage and re-discovery through MQDB on reconnect, and applies a `tc`/`netem` rate and delay profile (degraded/low-bandwidth) that the shared picture holds under. Loss shaping is not scripted: `netem` drops whole segmentation-offload buffers, so a valid loss figure needs UDP GSO disabled first. The HF/UHF/SATCOM profile values are placeholders pending real figures.

## Substrate (separate repositories)

The exchange and convergence layer this builds on is existing LabOverWire work:

- [MQDB](https://github.com/LabOverWire/MQDB): reactive document store with a native MQTT 5.0 broker.
- [stitch-rs](https://github.com/LabOverWire/stitch-rs): offline-first reactive sync, including `stitch-p2p` (broker-less multi-leader sync whose convergence is machine-checked in TLA+) and `mqp2p` (QUIC NAT traversal).
- [mqtt-lib](https://github.com/LabOverWire/mqtt-lib): MQTT 5.0 in Rust with QUIC transport.

## Status

Present: the canonical model, the codec layer with two formats, the codec-bounded stitch-p2p exchange (wire format in, shared picture out, re-exportable to any format), and a containerized harness (`crates/c2-node` plus `harness/`) in which three nodes discover each other through an MQDB broker, connect peer-to-peer over QUIC, converge, hold the shared picture under a `netem` rate and delay profile, and reconverge after a network partition (with a write injected during the outage) by re-discovering through MQDB. Next: loss shaping with UDP GSO disabled, and capturing convergence latency per bearer profile.

Discovery uses the MQDB broker (`mqp2p` peer registration over `$DB/peers`), with presence maintained by a heartbeat-renewed TTL lease so a node that goes away drops out of discovery rather than lingering as stale; the message exchange and convergence are peer-to-peer over QUIC, so the broker is not in the data path. A broker-less direct-dial path (`QuicEndpoint::connect`, no STUN, no hole-punching) is also available and covered by the `quic_exchange` test, for fully disconnected operation. Netem loss is only trustworthy with UDP segmentation offload disabled; bandwidth and delay are unaffected.

## Build

```
cargo test
cargo clippy --all-targets -- -D warnings
```

## Scope note

This is a bounded proof of concept, not a product. Format coverage is a representative subset, and the NATO Core Data Framework is addressed at the design level pending access to the specification.
