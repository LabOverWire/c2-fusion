# c2-fusion

A proof-of-concept for turning real-time operational data into standards-compliant Command and Control (C2) messages and exchanging them across nodes over contested, low-bandwidth links, keeping every node's view convergent when links drop and rejoin.

Prototype developed for the DND IDEaS Competitive Projects challenge W7714-248676/017 (RF Data Fusion for Command and Control), Component 1a. It is research-stage (TRL 2 to 3): the enabling substrate is mature and in production, and this repository is the challenge-specific R&D built on top of it.

## Design

A canonical, format-independent C2 message model with a pluggable codec layer. Military wire formats map onto the model as codecs, so support for a new or changed standard is a new codec, not a change to the core. This directly answers the challenge's requirement for a modular, extensible design that accommodates emerging standards.

- `crates/c2-model`: the canonical model. Contact report, situation report, and request for information, each tagged with an operational domain and functional service.
- `crates/c2-codec`: the `Codec` trait, a registry, and two working codecs. `mtf-xml` (an XML Message Text Format representation, in the family of NATO APP-11 XML-MTF) and `niem-json` (a NIEM-style JSON exchange). Both round-trip the same canonical message, which is what proves the format-agnostic claim.
- `crates/c2-exchange`: maps a C2 message onto the stitch-p2p document store and projects the converged store back into a shared C2 picture. Integration tests exchange messages between two nodes over a pipe and show the picture converges, including after a link partition and rejoin.
- `harness/`: the DDIL emulation environment. Three Linux containers on a shared network, with `tc`/`netem` bearer profiles for HF, UHF, and SATCOM, used to exercise the exchange under degraded and partitioned links.

## Substrate (separate repositories)

The exchange and convergence layer this builds on is existing LabOverWire work:

- [MQDB](https://github.com/LabOverWire/MQDB): reactive document store with a native MQTT 5.0 broker.
- [stitch-rs](https://github.com/LabOverWire/stitch-rs): offline-first reactive sync, including `stitch-p2p` (broker-less multi-leader sync whose convergence is machine-checked in TLA+) and `mqp2p` (QUIC NAT traversal).
- [mqtt-lib](https://github.com/LabOverWire/mqtt-lib): MQTT 5.0 in Rust with QUIC transport.

## Status

Present: the canonical model, the codec layer with two formats, the stitch-p2p exchange with shared-picture convergence across a partition, round-trip and convergence tests, and the emulation harness. Next: run the exchange between nodes over QUIC inside the netem harness with real bearer profiles, and connect a codec at the ingest boundary so messages arrive in a wire format and leave as the shared picture.

## Build

```
cargo test
cargo clippy --all-targets -- -D warnings
```

## Scope note

This is a bounded proof of concept, not a product. Format coverage is a representative subset, and the NATO Core Data Framework is addressed at the design level pending access to the specification.
