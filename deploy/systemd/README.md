# lifed systemd unit

Production-grade systemd service unit for the `lifed` facade-aggregator
daemon — the Life Runtime's public-plane facade hosting
`life.v1.{Agent, Events, Wallet, Identity}` and the admin-plane
`life.admin.v1.{Runtime, Saga, RoutingCache}`.

This unit is the M5 SHIPPED finalization deliverable (plan task E1). It
mirrors the hardening intensity of `lifegw.service` (the sibling edge
gateway) and `crates/life-kernel/deploy/systemd/soma.service` (the µVM
kernel daemon), and exceeds both on a few axes:

| Directive | lifed | lifegw | soma |
|---|---|---|---|
| `RestrictAddressFamilies` | `AF_UNIX AF_NETLINK` | `AF_UNIX AF_NETLINK AF_INET AF_INET6` | `AF_UNIX AF_NETLINK` |
| `PrivateNetwork` | **yes** | no (public TCP) | no (vsock) |
| `ProtectKernelLogs` | yes | yes | yes |
| `ProtectClock` | yes | yes | yes |
| `ProtectHostname` | yes | yes | yes |
| `ProtectProc` | `invisible` | `invisible` | `invisible` |
| `ProcSubset` | `pid` | `pid` | `pid` |
| `SystemCallFilter` | `@system-service ~@privileged @resources @reboot` | `@system-service ~@privileged @resources` | `@system-service ~@privileged @resources` |
| `CapabilityBoundingSet=` | empty (drop all) | empty | `CAP_SYS_ADMIN CAP_NET_ADMIN` (Phase 2 backend) |

`lifed` doesn't open public sockets (lifegw owns that) and doesn't need
hypervisor capabilities (soma owns that), so the unit is the **most
hardened daemon in the Life Runtime**.

## Quick install

```bash
# 1. Build the binary.
cargo build --release -p lifed
sudo cp target/release/lifed /usr/local/bin/lifed
sudo chmod 755 /usr/local/bin/lifed

# 2. Create the unprivileged user + groups.
sudo groupadd --system life-runtime
sudo groupadd --system life-admin
sudo useradd --system --no-create-home --gid life-runtime --shell /usr/sbin/nologin life-runtime
# Add operators who should be able to call admin RPCs:
sudo usermod -a -G life-admin <operator-username>

# 3. Create the config + state directories.
sudo mkdir -p /etc/lifed /var/lib/lifed /var/log/lifed
sudo chown -R life-runtime:life-runtime /var/lib/lifed /var/log/lifed
sudo chmod 0750 /var/lib/lifed /var/log/lifed
sudo chmod 0755 /etc/lifed   # config dir is world-readable; secrets go in env

# 4. Copy the example config and tune it.
sudo cp crates/life-runtime/lifed/lifed.example.toml /etc/lifed/config.toml
sudo chmod 0640 /etc/lifed/config.toml
sudo chown root:life-runtime /etc/lifed/config.toml
# Edit /etc/lifed/config.toml — the OPERATOR ATTENTION comments
# call out the fields you'll likely tune:
#   - admin_plane.unix_socket_group (default: life-admin)
#   - auth.dev_signer_enabled (MUST be false in prod)
#   - vigil.otlp_endpoint (your OTLP collector URL)

# 5. Install the systemd unit.
sudo cp deploy/systemd/lifed.service /etc/systemd/system/lifed.service

# 6. Reload systemd and start the daemon.
sudo systemctl daemon-reload
sudo systemctl enable --now lifed
```

## Verify the daemon is running

```bash
# Check service status.
systemctl status lifed

# Follow live logs (structured JSON via vigil → journald).
journalctl -u lifed -f

# Confirm the public + admin Unix sockets are accepting connections.
ls -la /run/life/life.sock /run/life/life-admin.sock

# Quick smoke check — call the admin healthcheck (requires life-admin group):
sudo -u <operator-with-life-admin-group> grpcurl -unix \
    -plaintext /run/life/life-admin.sock \
    life.admin.v1.Runtime/HealthCheck
```

## Readiness check

The unit uses `Type=simple` (see the sd_notify section below), so
systemd marks lifed as "active" immediately after the binary forks.
The reliable readiness check is to connect to the public UDS:

```bash
# Can the public socket be connected?
socat - UNIX-CONNECT:/run/life/life.sock < /dev/null
# Returns immediately if the socket is bound and accepting.
```

For programmatic readiness (e.g. in a Kubernetes / nomad probe), prefer
calling `Runtime.HealthCheck` over the admin UDS:

```bash
grpcurl -unix -plaintext \
    /run/life/life-admin.sock \
    life.admin.v1.Runtime/HealthCheck
```

## Substrate dependencies

`lifed` depends on every substrate UDS being available before it can
serve traffic. The unit's `After=` directive declares dependency on
`lago.service`, `haima.service`, `anima.service`, and `arcand.service`.
Operators must install + enable each substrate's systemd unit before
starting lifed:

```bash
sudo systemctl enable --now lago.service
sudo systemctl enable --now haima.service
sudo systemctl enable --now anima.service
sudo systemctl enable --now arcand.service
sudo systemctl enable --now soma.service   # if hosting µVMs
sudo systemctl enable --now lifed.service
```

If any substrate UDS is missing at boot, `lifed` aborts with
`LifedError::Substrate("missing UDS: …")` (per Spec C₂ §11.4 +
Sub-phase D's `--allow-mock-fallback` flag, which is **never set** in
production).

## sd_notify (Type=notify) — Sub-phase F follow-up

The unit currently uses `Type=simple` because `lifed` does not yet
emit `sd_notify` ready signaling. Migration to `Type=notify` is queued
for Spec C₆ (autonomic-as-Π for Life Runtime) when `lifed` adopts the
`sd_notify_ready()` call inside `bootstrap.rs`.

When that lands, swap:

```diff
-Type=simple
+Type=notify
+NotifyAccess=main
```

And in `lifed/src/bootstrap.rs`, after the public + admin listeners
bind successfully:

```rust
#[cfg(target_os = "linux")]
{
    // Signal systemd we're ready to accept connections.
    sd_notify::notify(false, &[sd_notify::NotifyState::Ready])?;
}
```

## Socket activation (lifed.socket) — future enhancement

`lifed` does not yet consume `LISTEN_FDS`, so the unit binds the UDS
socket itself rather than accepting it from systemd. Adding socket
activation requires:

1. Update `lifed/src/listener.rs` to detect `LISTEN_FDS` env (use the
   `libsystemd` or `systemd-socket-activation` crate)
2. Ship a sibling `deploy/systemd/lifed.socket` unit defining the
   `ListenStream=/run/life/life.sock` + `/run/life/life-admin.sock`
3. Update the `[Service]` section to remove the `ExecStartPre` /
   binding logic and rely on the inherited fds

This is a future enhancement, not a Sub-phase F blocker. Tracked
informally; ticket to be filed when an operator hits the
"slow-start under load" pain that socket activation fixes.

## Logging

`lifed` uses structured JSON via `vigil` → `tracing-subscriber` →
journald. Filter / query patterns:

```bash
# All warnings + errors from the last hour:
journalctl -u lifed --since "1 hour ago" --grep "level\":\"(WARN|ERROR)"

# Trace context for a specific session id:
journalctl -u lifed | jq -c "select(.fields.life_session_id == \"sess-XXX\")"

# Rate-limit / breaker observability (Spec C₂ §9.3 metric series):
journalctl -u lifed | grep "life.daemon.dispatch.count\|life.daemon.breaker_state"
```

For OTLP export (recommended for production):
1. Set `vigil.otlp_endpoint = "http://otel-collector:4317"` in
   `/etc/lifed/config.toml`
2. Restart lifed: `sudo systemctl restart lifed`
3. Verify exporter init in logs: `journalctl -u lifed | grep "OTLP exporter"`

The 15 canonical metric series (Spec C₂ §9.3) flow through:

| Series | Source |
|---|---|
| `life.daemon.dispatch.count` | `life-runtime-pool::PoolGuard::emit_metrics` |
| `life.daemon.dispatch.duration_ms` | `PoolGuard::emit_metrics` |
| `life.daemon.semaphore.inflight` | `PoolGuard::drop` |
| `life.daemon.breaker_state` | `PoolGuard::drop` |
| `life.daemon.handler.duration_ms` | `HandlerMetricsLayer` tower middleware |
| `life.daemon.cache.size` | `RoutingCache::insert_minimal/evict` |
| `life.daemon.cache.evictions_total` | `RoutingCache::evict` |
| `life.daemon.slow_stream_total` | `FanoutRegistry::broadcast` |
| `life.session.created_total` | `RoutingCache::insert_minimal` |
| `life.session.destroyed_total` | `RoutingCache::evict` |
| `life.session.active` | `RoutingCache::insert_minimal/evict` |
| `life.session.replay_seconds` | `RoutingCache::cold_start` |
| `life.saga.inflight` | `SagaDriver::run` |
| `life.saga.completed_total` | `SagaDriver::run` |
| `life.saga.compensation_failed_total` | `SagaDriver::run` |

## Hardening rationale

The unit drops all capabilities and restricts both syscalls and address
families to the absolute minimum lifed needs:

- **`CapabilityBoundingSet=` (empty)**: lifed never needs root power.
  All FS access is through the unprivileged `life-runtime` user.
- **`RestrictAddressFamilies=AF_UNIX AF_NETLINK`**: lifed only opens
  Unix sockets (substrate UDS, public + admin sockets) and netlink
  (systemd notifications, future).
- **`PrivateNetwork=yes`**: defense-in-depth — even a future code-path
  bug that tries to open a TCP socket physically can't, because the
  network namespace lacks INET / INET6 entirely.
- **`SystemCallFilter=@system-service ~@privileged @resources @reboot`**:
  blocks privileged syscalls (mount, bpf, ptrace), resource
  manipulation (setpriority, setrlimit), and reboot — none of which
  lifed legitimately uses.
- **`MemoryDenyWriteExecute=yes`**: prevents JIT'd code or shellcode
  from being marked executable. Rust's stable codegen never needs
  this; we drop it.
- **`ProtectProc=invisible` + `ProcSubset=pid`**: lifed sees only its
  own /proc/<pid> entries; can't introspect siblings.

Operators copying this unit to other Life Runtime daemons should keep
the hardening intensity. The two relaxations are:

- `lifegw` adds `AF_INET AF_INET6` to `RestrictAddressFamilies` and
  drops `PrivateNetwork=yes` (it owns the public TCP surface).
- `soma` adds `CAP_SYS_ADMIN CAP_NET_ADMIN` to its cap set during
  Phase 2 (Docker backend); drops them in Phase 3 when migrating to
  Cube (BPF) backend.

## Operator runbook (quick reference)

| Symptom | Diagnostic | Fix |
|---|---|---|
| `lifed` won't start, journal says `LifedError::Substrate("missing UDS")` | `ls /run/life/{lago,haima,anima,arcan}.sock` | Start the missing substrate's systemd unit. |
| Public-plane RPCs return `Status::unauthenticated` | `journalctl -u lifed --since 5m \| grep "JWKS"` | Verify `lifegw.service` is running and `auth.jwks_path` matches lifegw's publish path. |
| Admin-plane RPCs return `Status::permission_denied` | `id <operator-user>` | Check `<operator>` is in the `life-admin` group. |
| Breaker tripped (substrate flaky) | `journalctl -u lifed \| grep "breaker_state\":2"` (Open) | Investigate the substrate's logs. Lifed will retry every 10s (HalfOpen). |
| Saga compensation failures | `journalctl -u lifed \| grep "saga_compensation_failed"` | These are best-effort per Spec C₂ §4.2; investigate the substrate logs. |
| Routing cache exploding (memory pressure) | Admin RPC `RoutingCache.Dump` count | Tune `routing.max_sessions` down or `routing.idle_ttl_secs` lower. |

## References

- **Service unit**: `deploy/systemd/lifed.service`
- **Example config**: `crates/life-runtime/lifed/lifed.example.toml`
- **Config schema source**: `crates/life-runtime/lifed/src/config.rs`
- **Spec C₂** (lifed facade design): `docs/superpowers/specs/2026-04-26-spec-c2-lifed-facade.md`
- **M5 plan** (sub-phase E task E1): `docs/superpowers/plans/2026-04-26-m5-lifed-build.md`
- **Sibling units**: `deploy/systemd/lifegw.service`, `crates/life-kernel/deploy/systemd/soma.service`
