# lifegw-stack — Railway-shaped multi-process container.
#
# What runs inside:
#   - arcan          (substrate-plane gRPC UDS at /run/life/arcan.sock —
#                     Stage 5: lifed picks it up as the REAL arcan substrate)
#   - lagod          (substrate-plane gRPC UDS at /run/life/lago.sock —
#                     Stage 6: lifed picks it up as the REAL lago substrate;
#                     haima/anima stay mocked via LIFED_ALLOW_MOCK_FALLBACK)
#   - lifed          (UDS at /run/life/life.sock + /run/life/life-admin.sock)
#   - lifegw         (HTTPS on 127.0.0.1:8443, self-signed cert, dev signer)
#   - caddy          (binds :$PORT cleartext, reverse-proxies to https://127.0.0.1:8443
#                     with tls_insecure_skip_verify; preserves WS upgrade + h2c → h2 hop)
#
# Why this shape:
#   - Spec C wires lifegw → lifed via UDS (`/run/life/life.sock`). Railway services
#     are TCP-only across containers, so co-locating both daemons in one container
#     keeps the canonical wire shape intact.
#   - Railway terminates TLS at its edge and forwards plaintext HTTP to `:$PORT`,
#     while lifegw is HTTPS-only by design. Caddy bridges the two: cleartext on
#     the public side (Railway-edge already terminated TLS) and self-signed TLS
#     on the lifegw side. The detour exists purely to avoid forking lifegw with
#     a "behind-proxy cleartext" mode for one deploy target — see
#     `apps/chat/docs/superpowers/specs/2026-05-03-life-runtime-canonical.md` §M.
#
# Build context: the `core/life` workspace root (so the Cargo workspace is
# resolvable). Build command:
#   docker build -f deploy/railway/lifegw-stack/Dockerfile -t lifegw-stack .

# ── Stage 1 — Rust builder ──────────────────────────────────────────────────
FROM rust:1.93-bookworm AS builder
WORKDIR /build

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        protobuf-compiler libprotobuf-dev \
        clang lld pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# `prost-build` invokes protoc and looks up well-known protos
# (`google/protobuf/timestamp.proto` etc.) via an include path. The
# `libprotobuf-dev` package places them at `/usr/include/google/protobuf`
# on Debian; `PROTOC_INCLUDE` makes prost-build pass them as `-I` to protoc.
ENV PROTOC=/usr/bin/protoc
ENV PROTOC_INCLUDE=/usr/include

COPY . .

# Build the daemons in one pass. Default features pull in `kms-vault`
# (compiled but unused — runtime config selects `KmsProvider::Dev`) and
# `tokio-tungstenite/handshake` so the WS upgrade path is live.
# Stage 5: `-p arcan` ships the real arcan substrate daemon — its
# `serve --uds-socket /run/life/arcan.sock` binds the substrate-plane
# gRPC server that lifed's per-substrate bootstrap dials.
# Stage 6: `-p lagod` ships the real lago substrate daemon — its
# `--uds-socket /run/life/lago.sock` binds `lago.v1.LagoSubstrate` over
# the same socket lifed's lago-proxy dials.
RUN cargo build --release \
        --manifest-path Cargo.toml \
        -p lifegw \
        -p lifed \
        -p arcan \
        -p lagod \
 && strip ./.target/release/lifegw ./.target/release/lifed ./.target/release/arcan ./.target/release/lagod

# ── Stage 2 — runtime image ─────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Caddy via the official .deb (small, current). openssl for self-signed cert
# generation at boot. tini reaps zombies cleanly when caddy is PID 1.
# netcat-openbsd provides `nc -zU <socket>` — a real UDS connect probe used
# by entrypoint.sh to verify lifed has both bind()ed AND listen()ed before
# lifegw starts dialing. `[[ -S … ]]` alone is insufficient because bind()
# creates the socket file before listen() makes the kernel ready to queue
# connections. See BRO-1193.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg debian-keyring debian-archive-keyring \
        apt-transport-https openssl tini \
        netcat-openbsd \
 && curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/gpg.key \
      | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg \
 && curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt \
      > /etc/apt/sources.list.d/caddy-stable.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends caddy \
 && apt-get purge -y --auto-remove curl gnupg \
 && rm -rf /var/lib/apt/lists/*

# Runtime layout — every UDS lifegw + lifed + arcan + lagod touches
# lives under /run/life/. Both daemons' durable state lives under the
# mounted volume so it survives redeploys (entrypoint.sh creates the
# subdirs; NOT pre-created here because LIFE_STATE_DIR is a runtime
# mount point): lagod at ${LIFE_STATE_DIR}/lago (journal + blobs),
# arcan at ${LIFE_STATE_DIR}/arcan (session workspaces + local blob
# cache — Stage 6b moved arcan's event JOURNAL to lagod over HTTP, so
# the RedbJournal there is unused unless ARCAN_LAGO_URL=embedded). The
# legacy /var/lib/arcan default is kept as a writable fallback.
RUN groupadd --system life-runtime \
 && useradd  --system --gid life-runtime --home-dir /var/lib/life --create-home life \
 && mkdir -p /run/life /etc/lifegw/tls /etc/lifegw /etc/lifed /var/lib/arcan \
 && chown -R life:life-runtime /run/life /etc/lifegw /etc/lifed /var/lib/arcan

WORKDIR /opt/life

COPY --from=builder /build/.target/release/lifegw /usr/local/bin/lifegw
COPY --from=builder /build/.target/release/lifed  /usr/local/bin/lifed
COPY --from=builder /build/.target/release/arcan  /usr/local/bin/arcan
COPY --from=builder /build/.target/release/lagod  /usr/local/bin/lagod

# Blessed authored agents for arcan's FsAgentRegistry (spawn_agent
# fails-closed with unknown_agent when the registry is empty).
COPY agents/ /opt/life/agents/

COPY deploy/railway/lifegw-stack/lifegw.toml    /etc/lifegw/config.toml
COPY deploy/railway/lifegw-stack/lifed.toml     /etc/lifed/config.toml
COPY deploy/railway/lifegw-stack/Caddyfile      /etc/caddy/Caddyfile
COPY deploy/railway/lifegw-stack/entrypoint.sh  /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Railway sets PORT at runtime; default for local docker run.
# LIFED_ALLOW_MOCK_FALLBACK stays true: Stage 6's per-substrate
# selection picks up the REAL arcand AND lagod (their UDS sockets are
# bound before lifed starts — see entrypoint.sh) and mocks only
# haima/anima until their daemons ship.
ENV PORT=8080 \
    LIFEGW_CONFIG=/etc/lifegw/config.toml \
    LIFED_CONFIG=/etc/lifed/config.toml \
    LIFED_ALLOW_MOCK_FALLBACK=true \
    RUST_LOG=info,lifegw=info,lifed=info,arcan=info,arcand=info,lagod=info,lago_journal=info,lago_api=info \
    RUST_BACKTRACE=1

EXPOSE 8080

# tini guards orphan reaping; entrypoint.sh fans out the five processes
# (arcan, lagod, lifed, lifegw, caddy) and waits on caddy under a live
# trap so SIGTERM drains every child (see entrypoint §6).
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
