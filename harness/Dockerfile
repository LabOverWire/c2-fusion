# Build context is the c2-fusion workspace root. Sibling crates (stitch-p2p,
# mqp2p) are fetched as git dependencies during the build, so nothing outside
# this repo needs to be copied in.
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p c2-node

# Runtime: iproute2 provides tc/netem for bearer shaping (rate/delay/loss).
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends iproute2 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/c2-node /usr/local/bin/c2-node
ENTRYPOINT ["/usr/local/bin/c2-node"]
