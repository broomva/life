//! Signal handling + in-flight-dispatch drain.
//!
//! [`install_signal_handler`] wires SIGINT / SIGTERM and returns the receiver
//! side of a oneshot that fires on the first signal.  The oneshot is already
//! the type expected by the tonic `serve_with_incoming_shutdown` helper, so
//! callers can pass it directly.
//!
//! [`drain_in_flight`] polls an atomic in-flight counter until it reaches zero
//! or a deadline elapses.  The caller owns the counter; `LifeKernelService`
//! exposes one via `LifeKernelService::in_flight()`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::oneshot;

/// Install SIGINT / SIGTERM handlers.
///
/// Returns the receiver side of a oneshot that fires on the first signal
/// received.  The sender side is dropped after the signal arrives, which
/// automatically completes the receiver.
///
/// Only one task actually waits on the underlying signal streams; the oneshot
/// is a lightweight bridge so other futures can `select!` on it.
pub fn install_signal_handler() -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        wait_for_signal().await;
        // Sending `()` — the receiver completes.  Ignore errors if the
        // receiver has already been dropped (e.g. the caller gave up early).
        let _ = tx.send(());
    });

    rx
}

/// Poll the in-flight counter until it reads zero or the deadline expires.
///
/// Returns `Ok(())` if drained cleanly, or `Err(remaining)` if the deadline
/// fired with `remaining > 0` dispatches still active.
pub async fn drain_in_flight(counter: Arc<AtomicUsize>, deadline: Duration) -> Result<(), usize> {
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    let sleep = tokio::time::sleep(deadline);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            biased;
            // Check counter first on each tick.
            _ = interval.tick() => {
                let remaining = counter.load(Ordering::SeqCst);
                if remaining == 0 {
                    return Ok(());
                }
            }
            // Deadline fired.
            _ = &mut sleep => {
                let remaining = counter.load(Ordering::SeqCst);
                if remaining == 0 {
                    return Ok(());
                }
                return Err(remaining);
            }
        }
    }
}

/// Wait for the first SIGINT or SIGTERM.
///
/// Uses `tokio::signal::unix` which is available on all Unix targets including
/// macOS.  On non-Unix targets a `ctrl_c` fallback is used.
#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => {
            tracing::info!("received SIGINT — initiating graceful shutdown");
        }
        _ = sigterm.recv() => {
            tracing::info!("received SIGTERM — initiating graceful shutdown");
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    tracing::info!("received ctrl-c — initiating graceful shutdown");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn drain_returns_ok_when_counter_is_zero() {
        let counter = Arc::new(AtomicUsize::new(0));
        let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(200)).await;
        assert!(result.is_ok(), "zero counter must drain immediately");
    }

    #[tokio::test]
    async fn drain_returns_err_when_deadline_expires_with_positive_count() {
        let counter = Arc::new(AtomicUsize::new(3));
        // Short deadline so the test runs fast.
        let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(120)).await;
        match result {
            Err(remaining) => assert_eq!(remaining, 3, "expected 3 in-flight"),
            Ok(()) => panic!("expected drain timeout but got Ok"),
        }
    }

    #[tokio::test]
    async fn drain_returns_ok_when_counter_drops_before_deadline() {
        let counter = Arc::new(AtomicUsize::new(1));
        let counter_clone = Arc::clone(&counter);

        // Decrement the counter after a short delay (before the deadline).
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            counter_clone.fetch_sub(1, Ordering::SeqCst);
        });

        let result = drain_in_flight(Arc::clone(&counter), Duration::from_millis(500)).await;
        assert!(result.is_ok(), "counter dropped to zero before deadline");
    }
}
