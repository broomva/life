# soma systemd unit

Production-grade systemd service unit for the Life Agent OS kernel daemon.

## Quick install

```bash
# 1. Copy the binary (built with `cargo build --release -p soma`).
sudo cp target/release/soma /usr/local/bin/soma
sudo chmod 755 /usr/local/bin/soma

# 2. Create the config directory and copy the example config.
sudo mkdir -p /etc/soma
sudo cp crates/life-kernel/deploy/config/soma.example.toml /etc/soma/config.toml
# Edit /etc/soma/config.toml to suit your deployment.

# 3. Install the systemd unit.
sudo cp crates/life-kernel/deploy/systemd/soma.service /etc/systemd/system/soma.service

# 4. Reload systemd and start the daemon.
sudo systemctl daemon-reload
sudo systemctl enable --now soma
```

## Verify the daemon is running

```bash
# Check service status.
systemctl status soma

# Follow live logs (structured JSON via Vigil → journald).
journalctl -u soma -f

# Confirm the Unix socket is accepting connections.
ls -la /run/life/soma.sock
```

## Readiness check

Because the unit uses `Type=simple` (see the sd_notify section below),
systemd marks soma as "active" immediately after the process forks.
The reliable readiness check is:

```bash
# Can the socket be connected?
socat - UNIX-CONNECT:/run/life/soma.sock
# Or via the soma CLI:
soma ping
```

## Capability table per backend profile

The `CapabilityBoundingSet` and `AmbientCapabilities` lines in `soma.service`
must be adjusted when changing the active backend.

| Backend | Phase | Required capabilities | Notes |
|---------|-------|-----------------------|-------|
| `local` (Docker / nsjail) | 2 (current) | `CAP_SYS_ADMIN`, `CAP_NET_ADMIN` | Docker requires `CAP_SYS_ADMIN` for namespace creation; nsjail requires both. |
| `cube` (BPF-based) | 3 — BRO-859 | `CAP_BPF`, `CAP_NET_ADMIN` | eBPF programs require `CAP_BPF` (Linux 5.8+). `CAP_SYS_ADMIN` can be dropped. |
| `vercel` (HTTP API) | 4 — BRO-860 | none | All sandboxing is remote; remove both `Capability*` lines entirely. |

To apply a different profile, edit `/etc/systemd/system/soma.service`,
then run:

```bash
sudo systemctl daemon-reload && sudo systemctl restart soma
```

## Configuration

The daemon reads `$SOMA_CONFIG` at startup (default: `/etc/soma/config.toml`).
See `deploy/config/soma.example.toml` for the annotated reference.

Key knobs:

| Config key | Default | Description |
|------------|---------|-------------|
| `server.unix_socket` | `/run/life/soma.sock` | Unix socket path (must be under `RuntimeDirectory`). |
| `server.drain_secs` | `30` | Graceful shutdown drain deadline. |
| `lago.store` | `in_memory` | Use `redb` with `path = "/var/lib/soma/events.redb"` for persistence. |
| `backends.local` | `true` | Enable the Docker/nsjail local backend. |

## CLI usage

`soma` is a unified binary: with no subcommand (or with `daemon`) it runs the
kernel daemon; with operator subcommands it acts as a client over the Unix
socket. Examples (against the default `/run/life/soma.sock`):

```bash
soma create-vm --image alpine:3.19 --cpu 1 --mem 256
soma dispatch <vm-id> --cmd "echo hello"
soma list-vms
```

## Rollback

```bash
# Stop the daemon and disable it.
sudo systemctl disable --now soma

# Restore the previous binary.
sudo cp /usr/local/bin/soma.bak /usr/local/bin/soma

# Restore the previous config.
sudo cp /etc/soma/config.toml.bak /etc/soma/config.toml

# Re-enable.
sudo systemctl daemon-reload && sudo systemctl enable --now soma
```

## sd_notify / Type=simple rationale

The unit uses `Type=simple` rather than `Type=notify`.

`Type=notify` requires the daemon to call `sd_notify(0, "READY=1\n")` after
its listening socket is bound and ready to accept connections.  Wiring
`sd_notify` properly requires adding the `sd-notify` crate as a dependency
and calling it inside `listener::serve` after `UnixListener::bind` succeeds.

This integration is deferred to avoid expanding the dependency surface.
The trade-off is:

- `systemctl start soma` returns as soon as the process starts, NOT after
  the socket is ready.  A `systemctl start && soma ping` script must retry
  the ping until the socket appears.
- `systemctl reload-or-restart` may briefly show "active" while soma is still
  initialising.

The `soma ping` / socket existence check described above is the recommended
readiness probe until proper sd_notify integration lands.

## Verifying the unit file

If `systemd-analyze` is available on the target host:

```bash
systemd-analyze verify /etc/systemd/system/soma.service
```

Note: `systemd-analyze verify` is not available in all CI environments
(particularly minimal Docker images and macOS).  The unit file is designed to
pass verification on any systemd >= 240 host.

## Socket ownership

The `RuntimeDirectory=life` directive creates `/run/life/` owned by `root`.
If a non-root user or group needs to connect (e.g. the `soma` system group),
add a `SocketGroup` line to `soma.service` **or** set `User=` / `Group=`
in `[Service]`.

BRO-896 has a `chown` stub inside `listener/unix.rs` that sets socket group
ownership at bind time; `SocketGroup=` in the systemd unit is the production
path and supersedes the in-process stub.

Example addition to `[Service]`:

```ini
User=soma
Group=soma
SocketGroup=soma
```

Create the system user before enabling the unit:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin soma
```
