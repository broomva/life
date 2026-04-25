//! Bounded exponential backoff. Spec B.1 §9.2 — retries only apply to
//! idempotent RPCs (GET-shaped). Mutating calls MUST NOT auto-retry.

use std::time::Duration;

/// Backoff schedule: 100ms, 400ms, 1600ms (three attempts total).
pub const DEFAULT_BACKOFF_MS: [u64; 3] = [100, 400, 1_600];

/// Execute `f` up to `1 + delays_ms.len()` times, sleeping between retries.
///
/// Returns the first `Ok`, or the last `Err` if all attempts fail.
/// The first attempt is immediate; each subsequent attempt waits the
/// corresponding entry in `delays_ms`.
pub async fn retry<F, Fut, T, E>(mut f: F, delays_ms: &[u64]) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut last = None;
    for (i, delay) in std::iter::once(&0u64).chain(delays_ms.iter()).enumerate() {
        if *delay > 0 {
            tokio::time::sleep(Duration::from_millis(*delay)).await;
        }
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if i < delays_ms.len() => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("at least one attempt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn succeeds_on_third_try() {
        let tries = Arc::new(AtomicUsize::new(0));
        let t = tries.clone();
        let result: Result<u32, &str> = retry(
            || {
                let t = t.clone();
                async move {
                    let n = t.fetch_add(1, Ordering::SeqCst);
                    if n < 2 { Err("boom") } else { Ok(42) }
                }
            },
            &[1, 1],
        )
        .await;
        assert_eq!(result, Ok(42));
        assert_eq!(tries.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn exhausts_all_retries() {
        let tries = Arc::new(AtomicUsize::new(0));
        let t = tries.clone();
        let result: Result<u32, &str> = retry(
            || {
                let t = t.clone();
                async move {
                    t.fetch_add(1, Ordering::SeqCst);
                    Err("always fail")
                }
            },
            &[1, 1],
        )
        .await;
        assert!(result.is_err());
        // 1 initial + 2 retries = 3 total
        assert_eq!(tries.load(Ordering::SeqCst), 3);
    }
}
