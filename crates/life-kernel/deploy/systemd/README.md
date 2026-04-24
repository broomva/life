# lifed systemd unit

Production-grade systemd service unit for the Life Agent OS kernel daemon.

## Quick install

```bash
# 1. Copy the binary (built with `cargo build --release -p lifed`).
sudo cp target/release/lifed /usr/local/bin/lifed
sudo chmod 755 /usr/local/bin/lifed

# 2. Create the config directory and copy the example config.
sudo mkdir -p /etc/lifed
sudo cp crates/life-kernel/deploy/config/lifed.example.toml /etc/lifed/config.toml
# Edit /etc/lifed/config.toml to suit your deployment.

# 3. Install the systemd unit.
sudo cp crates/life-kernel/deploy/systemd/lifed.service /etc/systemd/system/lifed.service

# 4. Reload systemd and start the daemon.
sudo systemctl daemon-reload
sudo systemctl enable --now lifed
```

## Verify the daemon is running

```bash
# Check service status.
systemctl status lifed

# Follow live logs (structured JSON via Vigil → journald).
journalctl -u lifed -f

# Confirm the Unix socket is accepting connections.
ls -la /run/lifed/sock
```

## Readiness check

Because the unit uses `Type=simple` (see the sd_notify section below),
systemd marks lifed as "active" immediately after the process forks.
The reliable readiness check is:

```bash
# Can the socket be connected?
socat - UNIX-CONNECT:/run/lifed/sock
# Or via the lifectl CLI (BRO-902):
lifectl ping
```

## Capability table per backend profile

The `CapabilityBoundingSet` and `AmbientCapabilities` lines in `lifed.service`
must be adjusted when changing the active backend.

| Backend | Phase | Required capabilities | Notes |
|---------|-------|-----------------------|-------|
| `local` (Docker / nsjail) | 2 (current) | `CAP_SYS_ADMIN`, `CAP_NET_ADMIN` | Docker requires `CAP_SYS_ADMIN` for namespace creation; nsjail requires both. |
| `cube` (BPF-based) | 3 — BRO-859 | `CAP_BPF`, `CAP_NET_ADMIN` | eBPF programs require `CAP_BPF` (Linux 5.8+). `CAP_SYS_ADMIN` can be dropped. |
| `vercel` (HTTP API) | 4 — BRO-860 | none | All sandboxing is remote; remove both `Capability*` lines entirely. |

To apply a different profile, edit `/etc/systemd/system/lifed.service`,
then run:

```bash
sudo systemctl daemon-reload && sudo systemctl restart lifed
```

## Configuration

The daemon reads `$LIFED_CONFIG` at startup (default: `/etc/lifed/config.toml`).
See `deploy/config/lifed.example.toml` for the annotated reference.

Key knobs:

| Config key | Default | Description |
|------------|---------|-------------|
| `server.unix_socket` | `/run/lifed/sock` | Unix socket path (must be under `RuntimeDirectory`). |
| `server.drain_secs` | `30` | Graceful shutdown drain deadline. |
| `lago.store` | `in_memory` | Use `redb` with `path = "/var/lib/lifed/journal.redb"` for persistence. |
| `backends.local` | `true` | Enable the Docker/nsjail local backend. |

## Rollback

```bash
# Stop the daemon and disable it.
sudo systemctl disable --now lifed

# Restore the previous binary.
sudo cp /usr/local/bin/lifed.bak /usr/local/bin/lifed

# Restore the previous config.
sudo cp /etc/lifed/config.toml.bak /etc/lifed/config.toml

# Re-enable.
sudo systemctl daemon-reload && sudo systemctl enable --now lifed
```

## sd_notify / Type=simple rationale

The unit uses `Type=simple` rather than `Type=notify`.

`Type=notify` requires the daemon to call `sd_notify(0, "READY=1\n")` after
its listening socket is bound and ready to accept connections.  Wiring
`sd_notify` properly requires adding the `sd-notify` crate as a dependency
and calling it inside `listener::serve` after `UnixListener::bind` succeeds.

This integration is deferred to **BRO-903** (post-Phase-2) to avoid expanding
the Phase 2 dependency surface.  The trade-off is:

- `systemctl start lifed` returns as soon as the process starts, NOT after
  the socket is ready.  A `systemctl start && lifectl ping` script must retry
  the ping until the socket appears.
- `systemctl reload-or-restart` may briefly show "active" while lifed is still
  initialising.

The `lifectl ping` / socket existence check described above is the recommended
readiness probe until BRO-903 lands.

## Verifying the unit file

If `systemd-analyze` is available on the target host:

```bash
systemd-analyze verify /etc/systemd/system/lifed.service
```

Note: `systemd-analyze verify` is not available in all CI environments
(particularly minimal Docker images and macOS).  The unit file is designed to
pass verification on any systemd >= 240 host.

## Socket ownership

The `RuntimeDirectory=lifed` directive creates `/run/lifed/` owned by `root`.
If a non-root user or group needs to connect (e.g. the `lifed` system group),
add a `SocketGroup` line to `lifed.service` **or** set `User=` / `Group=`
in `[Service]`.

BRO-896 has a `chown` stub inside `listener/unix.rs` that sets socket group
ownership at bind time; `SocketGroup=` in the systemd unit is the production
path and supersedes the in-process stub.

Example addition to `[Service]`:

```ini
User=lifed
Group=lifed
SocketGroup=lifed
```

Create the system user before enabling the unit:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin lifed
```
