//! `warpDriveUpdates` subscription resolver (Lumen Task 20, BRO-XXXX).
//!
//! # Schema shape
//!
//! Per the verbatim Warp SDL (mirrored at
//! `warp_graphql/schema_mirror/schema.graphql`):
//!
//! ```graphql
//! type RootSubscription {
//!   """Equivalent to GetCloudObjectUpdates in v1."""
//!   warpDriveUpdates: WarpDriveUpdate!
//! }
//!
//! union WarpDriveUpdate =
//!     AmbientTaskUpdated
//!   | ObjectActionOccurred
//!   | ObjectContentUpdated
//!   | ObjectDeleted
//!   | ObjectMetadataUpdated
//!   | ObjectPermissionsUpdated
//!   | TeamMembershipsChanged
//! ```
//!
//! Of the seven union variants, `AmbientTaskUpdated` is the one the
//! Lumen agent-task flow targets:
//!
//! ```graphql
//! type AmbientTaskUpdated {
//!   taskId: ID!
//!   taskUpdatedTs: Time!
//! }
//! ```
//!
//! Per Lumen spec L6-D5, the upstream client mutates `createAgentTask`
//! (Task 18 — already shipped) to receive a `task_id`, then subscribes
//! to `warpDriveUpdates` and demultiplexes incoming union members. The
//! `AmbientTaskUpdated` variant signals "the task with this id just
//! had a state change; refetch via your usual queries". Streaming
//! agent output (per-token deltas, tool calls, approvals) is bridged
//! to lifed's `Agent.StreamSession` server-streaming RPC, where each
//! [`life.v1.AgentEvent`] becomes a `WarpDriveUpdate` frame.
//!
//! # Phase α scope
//!
//! This module ships the **subscription contract** — schema
//! introspection sees `warpDriveUpdates` resolved by a real
//! [`SubscriptionField`] (not the auto-stub), and the resolver
//! produces a Stream that emits at least one valid frame so the
//! `graphql-transport-ws` round-trip terminates cleanly.
//!
//! The frame is a placeholder `AmbientTaskUpdated` with empty
//! `taskId` and current-wall-clock `taskUpdatedTs`. It is **NOT**
//! sourced from `Agent.StreamSession` yet — the gRPC bridge lands in
//! Task 21 (arcan-core `warp_agent` adapter) alongside Task 22
//! (full agent loop wiring). Skeletoning here keeps the WS pump
//! exerciseable end-to-end without coupling Phase α delivery to the
//! arcan adapter.
//!
//! # Union dispatch invariant
//!
//! async-graphql's dynamic-schema requires every union-typed
//! [`FieldValue`] to carry `.with_type("VariantName")` so the
//! dispatcher can resolve which concrete variant the value
//! represents — see `tier1::resolve_create_agent_task` for the
//! mutation-side pattern. Emitted frames in this module therefore
//! always tag their concrete variant explicitly.

use async_graphql::{
    Value,
    dynamic::{FieldValue, ResolverContext, SubscriptionField, SubscriptionFieldFuture, TypeRef},
};
use futures::stream;

/// Build the real [`SubscriptionField`] for `warpDriveUpdates`.
///
/// Registered by [`crate::warp_graphql::adapter::WarpSchema::build`]
/// when it encounters the `RootSubscription` object in the SDL walk;
/// any sibling field on the subscription root falls through to the
/// auto-stub (currently no other fields exist upstream, so this is
/// purely defensive).
///
/// # Phase α stream shape
///
/// The resolver returns a stream that yields exactly **one**
/// `AmbientTaskUpdated` frame then completes. This satisfies the
/// async-graphql subscription contract (the WS pump receives a
/// frame, then the stream closes) and lets the `graphql-transport-ws`
/// client see a valid payload. Real streaming (multiple frames,
/// sourced from `Agent.StreamSession`) lands in Task 21+22.
pub fn field() -> SubscriptionField {
    SubscriptionField::new(
        "warpDriveUpdates",
        TypeRef::named_nn("WarpDriveUpdate"),
        |_rctx: ResolverContext<'_>| {
            SubscriptionFieldFuture::new(async move {
                // Phase α placeholder: one `AmbientTaskUpdated` frame
                // with an empty `taskId` + ISO-8601 wall-clock
                // timestamp for `taskUpdatedTs`. Task 21+22 replaces
                // this iter-stream with a `tokio_stream` bridge over
                // `lifed.Agent.StreamSession`, mapping each
                // `AgentEvent` to the appropriate `WarpDriveUpdate`
                // variant.
                //
                // The `Time` scalar is registered as
                // [`async_graphql::dynamic::Scalar::new("Time")`] in
                // `WarpSchema::build`, which accepts any
                // [`async_graphql::Value`] at runtime — we use an
                // RFC-3339 string here matching how the Warp client
                // expects timestamps.
                let now = chrono::Utc::now().to_rfc3339();
                let payload = serde_json::json!({
                    "taskId": "",
                    "taskUpdatedTs": now,
                });
                let value = Value::from_json(payload).map_err(|e| {
                    async_graphql::Error::new(format!(
                        "warpDriveUpdates: failed to encode AmbientTaskUpdated placeholder: {e}"
                    ))
                })?;
                // The `.with_type("AmbientTaskUpdated")` tag is
                // mandatory: async-graphql's dynamic dispatcher
                // cannot pick a union variant from an untagged
                // FieldValue and would error with
                // "remaining selection sets for an abstract type
                // require __typename".
                let frame = FieldValue::value(value).with_type("AmbientTaskUpdated");
                Ok(stream::iter(vec![Ok::<_, async_graphql::Error>(frame)]))
            })
        },
    )
}

#[cfg(test)]
mod tests {
    // The end-to-end subscription test exercises `field()` indirectly
    // via `WarpSchema::build()`, so no `super::*` imports are needed.
    use crate::warp_graphql::adapter::WarpSchema;
    use futures::StreamExt;

    /// The subscription must be registered with a real resolver (not
    /// the auto-stub) and emit at least one valid GraphQL frame
    /// within a bounded time window.
    ///
    /// Failure modes this guards against:
    /// * Schema construction picks the stub path → empty stream → the
    ///   `tokio::time::timeout` fires.
    /// * Union dispatch fails (missing `.with_type(...)`) → first
    ///   frame carries errors.
    /// * The dynamic SubscriptionField API isn't compatible with the
    ///   way we wire it from `WarpSchema::build` → schema build
    ///   panics earlier (covered by `schema_builds_from_sdl`).
    #[tokio::test]
    async fn warp_drive_updates_subscription_emits_at_least_one_frame() {
        let schema = WarpSchema::build().expect("schema builds");
        let req =
            async_graphql::Request::new(r#"subscription { warpDriveUpdates { __typename } }"#);
        let mut stream = schema.inner().execute_stream(req);
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("subscription emits within 5s")
            .expect("subscription emits at least one frame");

        assert!(
            first.errors.is_empty(),
            "subscription emitted error: {:?}",
            first.errors
        );

        // Spot-check the union dispatched correctly: the placeholder
        // emits the `AmbientTaskUpdated` variant.
        let json = first.data.into_json().unwrap_or_default();
        let typename = json["warpDriveUpdates"]["__typename"].as_str();
        assert_eq!(
            typename,
            Some("AmbientTaskUpdated"),
            "expected union dispatch to AmbientTaskUpdated; got: {:?}",
            typename
        );
    }
}
