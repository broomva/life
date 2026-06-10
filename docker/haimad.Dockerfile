# Multi-stage build for haimad agentic finance engine
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

RUN cargo build --release -p haimad

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/haimad /usr/local/bin/haimad

ENV RUST_LOG=info
EXPOSE 3003

CMD ["haimad"]
