# c2-fusion

A proof-of-concept for turning real-time operational data into standards-compliant Command and Control (C2) messages and exchanging them across nodes over contested, low-bandwidth links, keeping every node's view convergent when links drop and rejoin.

Prototype developed for the DND IDEaS Competitive Projects challenge W7714-248676/017 (RF Data Fusion for Command and Control), Component 1a. It is research-stage (TRL 2 to 3): the enabling substrate is mature and in production, and this repository is the challenge-specific R&D built on top of it.

## Design

A canonical, format-independent C2 message model with a pluggable codec layer. Military wire formats map onto the model as codecs, so support for a new or changed standard is a new codec, not a change to the core. This directly answers the challenge's requirement for a modular, extensible design that accommodates emerging standards.

- `crates/c2-model`: the canonical model. Contact report, situation report, request for information, and own-unit position report, each tagged with an operational domain and functional service.
- `crates/c2-codec`: the `Codec` trait, a registry, and six working codecs: `niem-xml` and `niem-json` (NIEM 6.0 messages in the XML and JSON-LD representations, validated in tests against the official OASIS schemas plus a c2-fusion extension schema, and against the JSON schema generated from them; see `crates/c2-codec/schemas/niem/`), `coie-text` (slash-delimited character-oriented messages built on the public ADatP-3 / US MTF structure, not APP-11 conformant; see `docs/coie-text.md`), `mtf-xml` (a flat XML representation standing in for NATO APP-11 XML-MTF, not conformant), `c2-json` (the canonical model as plain JSON), and `compact-bin` (a bebytes binary encoding sized for low-bandwidth radio). All six round-trip the same canonical message, which is what proves the format-agnostic claim. A standards-conformant APP-11 / ADatP-3 codec is the next work. NCDF is addressed at the design level in [docs/ncdf-alignment.md](docs/ncdf-alignment.md), since its specification is not public.
- `crates/c2-exchange`: the ingest and egress boundary. A wire-format message is decoded by a codec and published to the stitch-p2p store; the converged store projects back into a shared C2 picture that can be re-exported in any codec's format. Integration tests show two nodes receiving different wire formats converge to one picture, that picture re-exports in a single format, and the picture reconverges after a link partition and rejoin.
- `crates/c2-node`: the node binary. It registers with an MQDB broker under a heartbeat-renewed presence lease, discovers peers through it (`$DB/peers`), and lets `stitch_p2p::Swarm` bridge each discovered peer into the sync session over QUIC.
- `harness/`: the DDIL emulation environment. An MQDB broker container plus three node containers on a shared network. The demo shows nodes discovering each other through MQDB and converging, then partition and rejoin (denied/intermittent) with a write injected during the outage and re-discovery through MQDB on reconnect, and applies a `tc`/`netem` rate and delay profile (degraded/low-bandwidth) that the shared picture holds under. Loss shaping is not scripted: `netem` drops whole segmentation-offload buffers, so a valid loss figure needs UDP GSO disabled first. The HF/UHF/SATCOM profile values are placeholders pending real figures.

## Substrate (separate repositories)

The exchange and convergence layer this builds on is existing LabOverWire work:

- [MQDB](https://github.com/LabOverWire/MQDB): reactive document store with a native MQTT 5.0 broker.
- [stitch-rs](https://github.com/LabOverWire/stitch-rs): offline-first reactive sync, including `stitch-p2p` (broker-less multi-leader sync whose convergence is machine-checked in TLA+) and `mqp2p` (QUIC NAT traversal).
- [mqtt-lib](https://github.com/LabOverWire/mqtt-lib): MQTT 5.0 in Rust with QUIC transport.

## Background research

The QUIC transport choice is informed by the author's research on MQTT over QUIC under loss (implemented in `mqtt-lib`):

- Preprint: Fabrício Bracht, "Evaluating Stream Mapping Strategies for MQTT over QUIC," SSRN 2026 (preprint, under peer review): https://papers.ssrn.com/sol3/papers.cfm?abstract_id=7576577
- Data and analysis scripts (open): Zenodo, https://doi.org/10.5281/zenodo.19098819

## Status

Present: the canonical model, the codec layer with six formats (including a NIEM 6.0 exchange in XML and JSON-LD validated against the official schemas, and a character-oriented text form), the codec-bounded stitch-p2p exchange (wire format in, shared picture out, re-exportable to any format), and a containerized harness (`crates/c2-node` plus `harness/`) in which three nodes discover each other through an MQDB broker, connect peer-to-peer over QUIC, converge, hold the shared picture under a `netem` rate and delay profile, and reconverge after a network partition (with a write injected during the outage) by re-discovering through MQDB. The same node also runs on Milk-V Duo S boards with a real LoRa bearer and GNSS time (`deploy/duo-s/`). Next: standards-conformant codecs, loss shaping with UDP GSO disabled, and capturing convergence latency per bearer profile.

Discovery uses the MQDB broker (`mqp2p` peer registration over `$DB/peers`), with presence maintained by a heartbeat-renewed TTL lease so a node that goes away drops out of discovery rather than lingering as stale; the message exchange and convergence are peer-to-peer over QUIC, so the broker is not in the data path. A broker-less direct-dial path (`QuicEndpoint::connect`, no STUN, no hole-punching) is also available and covered by the `quic_exchange` test, for fully disconnected operation. Netem loss is only trustworthy with UDP segmentation offload disabled; bandwidth and delay are unaffected.

## Build

```
cargo test
cargo clippy --all-targets -- -D warnings
```

For a full run-and-verify walkthrough (building, the DDIL harness, capturing evidence, and how to confirm each claim yourself), see [docs/USER_GUIDE.md](docs/USER_GUIDE.md).

## Scope note

This is a bounded proof of concept, not a product. Format coverage is a representative subset, and the NATO Core Data Framework is addressed at the design level pending access to the specification.
