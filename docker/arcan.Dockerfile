# Multi-stage build for arcan agent runtime daemon
# Build context: repository root (life/)

FROM rust:latest AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifest and lockfile first (Docker layer caching)
COPY Cargo.toml Cargo.lock ./

# Copy all crate Cargo.toml files (for dependency resolution)
COPY crates/ crates/

# Build release binary
RUN cargo build --release -p arcan

# Runtime stage
FROM debian:trixie-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl bubblewrap && \
    rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash arcan

COPY --from=builder /build/target/release/arcan /usr/local/bin/arcan

USER arcan
WORKDIR /home/arcan

ENV RUST_LOG=info
EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

ENTRYPOINT ["arcan", "serve"]
