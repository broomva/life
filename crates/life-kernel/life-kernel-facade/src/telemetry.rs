//! Vigil span helpers. The facade emits only `life.facade.*` spans —
//! it does **not** write Lago events (see Spec B.1 §9.3).

/// Build the canonical span name for a facade RPC.
pub fn span_name(service: &str, op: &str) -> String {
    format!("life.facade.{service}.{op}")
}

/// `tracing::info_span!` wrapper so all facade RPCs share a shape.
///
/// # Examples
///
/// ```
/// let span = life_kernel_facade::facade_span!("events", "head");
/// ```
#[macro_export]
macro_rules! facade_span {
    ($service:expr, $op:expr) => {
        tracing::info_span!(
            "life.facade",
            otel.name = %$crate::telemetry::span_name($service, $op),
            service = $service,
            op = $op,
        )
    };
    ($service:expr, $op:expr, $($k:tt = $v:expr),+ $(,)?) => {
        tracing::info_span!(
            "life.facade",
            otel.name = %$crate::telemetry::span_name($service, $op),
            service = $service,
            op = $op,
            $($k = $v),+
        )
    };
}
