# Multi-stage build for lagod persistence daemon
# Build context: repository root (life/)

FROM rust:1.93-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
# All workspace members (crates/ + apps/) must be present for cargo to
# resolve the workspace manifest; proto/ is read by build scripts
# relative to the repo root.
COPY crates/ crates/
COPY apps/ apps/
COPY proto/ proto/

RUN cargo build --release -p lagod

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/lagod /usr/local/bin/lagod
COPY crates/lago/default-policy.toml /etc/lago/policy.toml
COPY crates/lago/lago.toml /etc/lago/lago.toml

ENV LAGO_DATA_DIR=/data/.lago
ENV RUST_LOG=info

EXPOSE 8080 50051

CMD ["lagod", "--data-dir", "/data/.lago", "--http-port", "8080", "--grpc-port", "50051", "--config", "/etc/lago/lago.toml"]
