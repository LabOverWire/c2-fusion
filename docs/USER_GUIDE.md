# c2-fusion: running and verifying the proof of concept

This guide explains how to build and run the proof of concept, what each step should produce, and how to confirm every claim yourself rather than taking the output on trust. It is written for someone who did not build the code and wants to check that it does what the proposal says.

## What the POC demonstrates

- A canonical, format-independent C2 message model with a pluggable codec layer, so a wire format is a codec rather than a change to the core.
- Round-trip fidelity: the same canonical message survives encode and decode through each wire format (MTF-XML and NIEM-JSON in this POC).
- A codec-bounded exchange: two nodes that ingest different wire formats converge to one shared picture and can re-export it in a single format.
- Discovery and presence through an MQDB broker: nodes register under `$DB/peers`, find each other, and maintain a heartbeat-renewed TTL lease.
- Peer-to-peer convergence over QUIC whose safety property is machine-checked in TLA+ (in the sibling `stitch-rs` repo).
- DDIL behaviour in a containerised harness: converge, hold under a degraded link, partition with a write injected during the outage, and reconverge on reconnection, with presence reflecting the partition.

What the POC does not claim: the codecs are representative stand-ins, not full APP-11 / ADatP-3 / NIEM / NCDF conformance; there is no RF ingest; and the bearer profile numbers are placeholders. Those are the funded work, not present capability.

## Prerequisites

- Rust (stable) with cargo. Install via rustup (rustup.rs).
- Docker, for the harness. The nodes and broker run as Linux containers.
- The sibling MQDB repository checked out next to this one, because the harness builds the broker image from it. `harness/run.sh` expects it at `../../MQDB` relative to the c2-fusion repo (so if c2-fusion is at `~/repos/c2-fusion`, MQDB must be at `~/repos/MQDB`).
- Internet access on the first build. The sibling crates (`stitch-p2p`, `mqp2p`) are fetched as git dependencies from github.com/LabOverWire, pinned in `Cargo.lock`.

Platform note: `tc`/`netem` shaping runs inside the Linux containers, not on the host, so the harness behaves the same on macOS and Linux as long as Docker is available.

## Repository layout

- `crates/c2-model`: the canonical message model (contact report, situation report, request for information), each tagged with an operational domain and functional service.
- `crates/c2-codec`: the `Codec` trait, a registry, and two codecs, `mtf-xml` and `niem-json`.
- `crates/c2-exchange`: the ingest and egress boundary over the stitch-p2p store.
- `crates/c2-node`: the node binary that registers with an MQDB broker, discovers peers, and bridges them into the sync session over QUIC.
- `harness/`: the DDIL emulation environment (broker plus three nodes) and the scripts described below.

## Part 1: library build and unit tests (no Docker)

From the repository root:

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

What to look for: every test binary reports `test result: ok`, clippy finishes with no warnings, and fmt reports no diff. These are the same checks CI runs, so if they pass locally they pass in CI.

What the key tests prove, and how to confirm:

- Format-agnostic round-trip (`crates/c2-codec`): `contact_report_round_trips_with_special_chars_and_floats`, `rfi_round_trips`, `sitrep_round_trips_with_integer_field`, and `same_message_round_trips_through_every_registered_codec`. These encode a canonical message to a wire format and decode it back, asserting equality. Confirm it yourself by running `cargo test -p c2-codec` and reading the passing names; to see a test actually exercise the claim, open `crates/c2-codec/src/mtf_xml.rs` or `niem.rs` and read the round-trip assertion.
- Codec registry behaviour: `registry_exposes_both_formats`, `unknown_format_is_none`, `missing_root_is_decode_error`. These confirm the registry lists the two formats and rejects unknown or malformed input rather than guessing.
- Codec-bounded convergence (`crates/c2-exchange/tests/codec_ingest.rs`): `two_wire_formats_converge_to_one_picture_and_reexport` ingests one message in MTF-XML and another in NIEM-JSON on two separate stores, syncs them, and asserts both reach the same picture and can re-export it in one format. This is the core "different formats in, one shared picture out" claim. Run `cargo test -p c2-exchange` and confirm it passes; read the test body to see the two formats go in and the single re-export come out.

If any check fails, that is a real failure to investigate, not noise. The suite is expected to be green.

## Part 2: the DDIL harness (Docker)

### Run the end-to-end demo

```
cd harness
./run.sh
```

`run.sh` builds three images (the MQDB broker from the sibling repo, an Alpine-repackaged broker with dev credentials, and the `c2-node` image), brings up a broker and three nodes (edge, relay, hq) on a Docker bridge, and drives the full sequence: discover through MQDB, converge over QUIC, apply a `netem` degraded profile to edge, partition edge, inject a new contact while it is isolated, then heal and reconverge. It tears the stack down at the end.

What success looks like (the tail of the output):

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

If the broker is slow to start on the very first run, the scripts wait and retry; a single cold-start retry is normal.

### Capture detailed evidence

```
./evidence.sh
```

This runs the same sequence but prints, at each phase, the MQDB peer registry and each node's shared picture (its message-id set), so you can see the mechanism rather than a summary. The phases are:

- PHASE A (converged): the registry lists three peers, each node logs `REGISTERED`, and all three nodes show the same three-message picture (`C-1,R-1,S-1`).
- PHASE B (edge partitioned, held past the lease): edge shows a four-message picture including `C-2`, hq still shows three; the registry shows edge's lease lapsed while relay and hq stay fresh (see the next section on reading `_expires_at`).
- PHASE C (reconverged): all three nodes show the same four-message picture (`C-1,C-2,R-1,S-1`), and the registry shows all leases fresh again.

## Part 3: confirming each claim yourself

For each claim below, run the command and check the stated signal. The point is that the evidence is observable, not asserted.

1. Discovery is really through MQDB. In `evidence.sh` PHASE A, the registry (queried from the broker, not the nodes) lists three peer records under `$DB/peers`, and each node prints `REGISTERED name=... peer_id=... broker=mqdb:1883`. If discovery were faked, the broker-side registry would be empty.
2. Convergence is peer-to-peer over QUIC, broker not in the data path. The nodes reach the same picture, and the design keeps the broker for discovery and signalling only. You can confirm the broker is not carrying message traffic by noting the nodes keep their picture through the partition of edge from the broker, and by reading `crates/c2-node/src/main.rs`, where the data plane is the stitch-p2p swarm over QUIC, not MQTT publishes.
3. The picture holds under a degraded link. `run.sh` applies a `netem` rate-and-delay profile to edge and then confirms hq still shows count=3. The shaping command and the hold check are visible in `run.sh`.
4. A partition isolates a node and the injected write stays local. During PHASE B, edge reaches count=4 (it ingested `C-2` while cut off) and hq stays count=3, proving the write did not leak across the partition.
5. Presence reflects the partition. In PHASE B the registry shows edge's lease lapsed (its `_expires_at` is in the past and its `_version` is frozen) while relay and hq keep renewing. See the next section for how to read this.
6. Reconvergence after heal. In PHASE C all three nodes reach the identical four-id set, and edge's lease is fresh again, proving it re-discovered and caught up.
7. Format-agnostic exchange. `cargo test -p c2-exchange` passes `two_wire_formats_converge_to_one_picture_and_reexport`; read the test to see two different wire formats converge to one picture.
8. Convergence is machine-checked. The TLA+ specifications live in the sibling repo at `stitch-rs/crates/stitch-p2p/spec/`. Its `README.md` records the core safety property `InvConvergence` (fully synchronised peers hold identical state) checked exhaustively at 10,405 states for the base configuration, with a deliberately unsafe variant producing the expected counterexample. If you have the TLA+ tools (TLC) installed, you can re-run `StitchP2P.tla` against `StitchP2P.cfg` to reproduce the state count.

## Reading the MQDB peer registry (how to interpret `_expires_at`)

Each peer record carries `_expires_at`, a unix-seconds lease. A peer is live only while `_expires_at` is greater than the current time. `evidence.sh` prints `now=<epoch>` just above each registry dump so you can compare directly.

The nodes here use a six-second demo lease and renew it on a heartbeat. While a node is connected, its `_expires_at` keeps advancing ahead of `now` and its `_version` keeps incrementing. When edge is partitioned it can no longer renew, so its `_expires_at` stops advancing and falls behind `now` (the lease has lapsed) and its `_version` freezes. Relay and hq, still connected, show `_expires_at` in the future and rising `_version`. On reconnection edge renews, so its `_expires_at` and `_version` advance again. That contrast, lapsed versus fresh in the same dump, is the presence mechanism working.

The broker also sweeps expired records on its own timer (roughly every 60 seconds), which is coarser than the lease, so the authoritative fast signal is the client-side check that `_expires_at` is still in the future.

## Bearer profiles (optional)

`harness/netem-profiles.sh <interface> <hf|uhf|satcom|clear>` applies an emulated tactical-bearer profile with `tc`/`netem`. Run it inside a container (it needs `NET_ADMIN`), not on the host. The bandwidth, delay, and loss values are placeholders pending real HF/UHF/SATCOM figures; treat them as illustrative. Loss in particular is only trustworthy with UDP segmentation offload disabled, so the harness applies rate and delay, not scripted loss.

## Troubleshooting

- The broker image fails to build: confirm the sibling MQDB repo is at `../../MQDB` relative to c2-fusion.
- Docker runs out of space during a build: `docker system prune` to reclaim space, then retry.
- The first broker start times out: re-run; a single cold-start retry is expected, and the scripts already wait for the broker to answer before proceeding.
- Before pushing changes, run `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings`; CI runs the same and the portal forms are unrelated to these.

## Cleanup

The scripts tear the stack down automatically. To clean up manually:

```
docker compose -f harness/docker-compose.yml down -v --remove-orphans
docker image prune
```

## Substrate repositories

The exchange and convergence layer this builds on is separate open-source work:

- stitch-rs (stitch-p2p convergence engine with the TLA+ specs, and mqp2p QUIC transport and MQDB-based discovery): github.com/LabOverWire/stitch-rs
- MQDB (the discovery and signalling substrate): github.com/LabOverWire/MQDB
- mqtt-lib (MQTT 5.0 in Rust with QUIC transport): github.com/LabOverWire/mqtt-lib
