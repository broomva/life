# Multi-stage build for arcan agent runtime daemon
# Build context: repository root (life/)

FROM rust:1.93-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifest and lockfile first (Docker layer caching)
COPY Cargo.toml Cargo.lock ./

# All workspace members (crates/ + apps/) must be present for cargo to
# resolve the workspace manifest; proto/ is read by *-substrate-proto,
# aios-proto, and life-*-proto build scripts relative to the repo root.
COPY crates/ crates/
COPY apps/ apps/
COPY proto/ proto/

# Build release binary
RUN cargo build --release -p arcan

# Runtime stage
FROM debian:trixie-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl bubblewrap && \
    rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash arcan

COPY --from=builder /build/target/release/arcan /usr/local/bin/arcan

# Blessed authored agents (agents/*.md) — FsAgentRegistry loads ./agents
# relative to the workdir; without them spawn_agent returns unknown_agent.
COPY --chown=arcan:arcan agents/ /home/arcan/agents/

# Blessed runtime skill set — SkillRegistry scans ARCAN_SKILLS_DIR first so
# discovery does not depend on CWD/HOME (skills_found=0 otherwise). See
# runtime-skills/README.md for the zero-tool blessed-set policy. BRO-1469.
COPY --chown=arcan:arcan runtime-skills/ /home/arcan/skills/

USER arcan
WORKDIR /home/arcan

ENV RUST_LOG=info
ENV ARCAN_SKILLS_DIR=/home/arcan/skills
EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

ENTRYPOINT ["arcan", "serve"]
