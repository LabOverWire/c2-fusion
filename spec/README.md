# Pull suppression in stitch-p2p sessions

`SessionPull.tla` models the periodic pull of a stitch-p2p sync session (`crates/stitch-p2p/src/session.rs` in stitch-rs) to check two changes before they are implemented: fewer messages between peers that are already in sync, and a different pull interval for each session.

## What is modelled

Each peer appends frames to its own log (`Write`) and pushes each new frame to every other peer. The pull runs per ordered pair of peers: `Tick` sends a `Hello` carrying the sender's cursors, the receiver answers with a `Delta` holding the frames the sender lacks, and a `Delta` raises the receiver's cursors. A pushed frame is applied only if it is the next one in its origin's log, as `SyncState::receive` does with a gap. Every message can be lost (`Drop`), and each channel slot holds at most the latest message of its kind, so a newer `Hello` or `Delta` replaces an undelivered older one, the same as losing the older one. Record values and last-writer-wins are not modelled: `StitchP2P.tla` in stitch-rs already shows that peers holding the same frames hold the same state, so here convergence means every peer holds every frame (`AllSynced`).

Fairness: a peer allowed to send a `Hello` eventually does (weak fairness on `Tick`), and a message that is sent again and again is eventually delivered (strong fairness on each delivery), so loss is unbounded but not permanent. `Converges` (`<>[]AllSynced`) states that once writing stops, every peer ends up with every frame.

## Modes

- `baseline` is today's protocol: both sides of a session send a `Hello` every interval, and every `Hello` gets a `Delta` back, even an empty one.
- `initiator` is the proposed change: only the peer with the lower id sends the periodic `Hello`; a `Delta` is sent only when it is not empty; and a peer that receives a `Hello` showing that the sender holds frames it lacks sends its own `Hello` back, so the pull still works in both directions.
- `naive` skips a periodic `Hello` when the sender's own cursors have not changed since its last one, and otherwise behaves like `initiator` without the `Hello` sent back.

## Results

Checked on 2026-10-18 with the `tla` model checker. `Writers` limits which peers write, which keeps the three-peer models small enough to finish; with a single writer, peer 1 initiates both of its sessions, peer 2 initiates one, and peer 3 none.

| Mode | Peers | Writers, frames each | States | Invariants | `Converges` |
|---|---|---|---|---|---|
| `baseline` | 2 | both, 2 | 161,018 | hold | holds |
| `initiator` | 2 | both, 2 | 29,565 | hold | holds |
| `naive` | 2 | both, 2 | 48,739 explored | hold | violated |
| `initiator` | 3 | peer 1, 1 | 12,911 | hold | holds |
| `initiator` | 3 | peer 2, 1 | 8,689 | hold | holds |
| `initiator` | 3 | peer 3, 1 | 5,161 | hold | holds |
| `naive` | 3 | peer 3, 1 | 1,090 explored | hold | violated |

Not checked: `initiator` at three peers with two writers (peers 2 and 3), and `baseline` at three peers with any single writer. These runs stopped at their time limits (100 seconds, about 300,000 states; 600 seconds and 1.69 million states for `baseline` with peer 3 writing), so their results are unknown. `baseline` sends both periodic `Hello` messages and every `Delta`, so its state space is far larger than that of `initiator`.

`naive` fails the same way at both sizes: the writer's pushes are lost, and since no peer's own cursors change again, no peer sends a `Hello`, so the others never receive the frame. Suppressing a pull because nothing changed locally is unsafe whenever pushes can be lost. `initiator` avoids it because the periodic `Hello` never stops, and the `Hello` sent back closes the case where the side holding the frame is not the one sending periodic `Hello` messages.

## Per-session intervals

The model has no clock: each session's `Tick` is fair on its own, so the results hold for any positive interval per session, and a node can pull a peer on a fast bearer every few seconds and a peer on HF every few minutes without affecting convergence. The interval bounds how long catch-up takes after a lost push, which this model does not measure.

## Cost

In a session whose peers are in sync, `baseline` sends two `Hello` and two empty `Delta` messages per interval, which is about 6 packets with QUIC acknowledgements (measured in `harness/README.md`, HF idle traffic by source). `initiator` sends one `Hello` and nothing back.

## Checking

Run TLC on `SessionPull.tla` with one of the `SessionPull_<mode>_<size>.cfg` files; each checks `TypeOK`, `InvCursorBound` and `InvOwnLog` and the property `Converges`.
