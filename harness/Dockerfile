# Build context must contain stitch-rs/ and c2-fusion/ at its root, in the same
# relative layout as the repos on disk, so the c2-fusion path dependencies on
# ../../../stitch-rs/crates/{stitch-p2p,mqp2p} resolve. harness/run.sh assembles
# such a context in a temp staging directory.
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY stitch-rs/crates/stitch-p2p stitch-rs/crates/stitch-p2p
COPY stitch-rs/crates/mqp2p stitch-rs/crates/mqp2p
COPY c2-fusion c2-fusion
WORKDIR /build/c2-fusion
RUN cargo build --release -p c2-node

# Runtime needs only glibc: QUIC uses fingerprint-pinned TLS (no CA store) and
# the core demo (partition/rejoin) needs no tc. netem shaping is applied only if
# the image happens to carry iproute2; the orchestration script skips it if not.
FROM debian:bookworm-slim
COPY --from=builder /build/c2-fusion/target/release/c2-node /usr/local/bin/c2-node
ENTRYPOINT ["/usr/local/bin/c2-node"]
