# Multi-stage build for autonomicd homeostasis controller
# Build context: repository root (life/)

FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p autonomicd

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/autonomicd /usr/local/bin/autonomicd

ENV RUST_LOG=info
EXPOSE 3002

CMD ["autonomicd"]
