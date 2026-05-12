//! Batched lifed gRPC clients.
//!
//! Each lifed service we call from resolvers is accessed through a
//! tonic gRPC client. For services that benefit from N+1 coalescing
//! (e.g., a query that needs `user` for 50 message authors in one
//! response), wrap them later in `async_graphql::dataloader::DataLoader`
//! — the framework's built-in lightweight loader (futures-channel + lru).
//! v1 holds the raw tonic clients without batching; Task 8 ships only
//! the holder shape.
//!
//! ## Wired clients
//!
//! Per Lumen spec L6-D5 the plan template listed 7 service clients;
//! lifed M5 actually exposes 4 public-plane services (`Agent`, `Events`,
//! `Wallet`, `Identity`) plus 3 admin-plane services (`Runtime`, `Saga`,
//! `RoutingCache`). All seven exist in `life-runtime-proto` and are
//! bundled here, sharing a single gRPC channel via `Arc<LifedDataloaders>`
//! in the per-request context.
//!
//! `Inference` and `Settings` from the plan template's original sketch
//! do NOT exist in lifed — there is no `life.v1.Inference` or
//! `life.v1.Settings` proto. Tier-3 resolvers (Task 9) provide safe
//! defaults for any GraphQL fields backed by services that don't yet
//! have a lifed surface.

use tonic::transport::Channel;

// gRPC clients (proto types live in `life-runtime-proto`).
// Public-plane services per Spec C₂ §3 (lifed facade).
use life_runtime_proto::life::v1::{
    agent_client::AgentClient, events_client::EventsClient, identity_client::IdentityClient,
    wallet_client::WalletClient,
};
// Admin-plane services per Spec C₂ §7 (operator surface). Mounted here
// so future operator-facing GraphQL fields (e.g., `Mutation.evictRoute`)
// can dispatch without re-dialing.
use life_runtime_proto::life::admin::v1::{
    routing_cache_client::RoutingCacheClient, runtime_client::RuntimeClient,
    saga_client::SagaClient,
};

/// Tonic clients for each lifed service we call from warp_graphql
/// resolvers. Per Lumen spec L6-D5, all available services are bundled
/// into one struct shared via `Arc<LifedDataloaders>` in the per-request
/// context.
///
/// Real DataLoader-style batching is deferred to async-graphql's
/// built-in `dataloader` feature; this struct is currently a holder.
#[derive(Clone)]
pub struct LifedDataloaders {
    /// `life.v1.Agent` — session lifecycle + dispatch + approval.
    pub agent: AgentClient<Channel>,
    /// `life.v1.Events` — event stream + blob fetch.
    pub events: EventsClient<Channel>,
    /// `life.v1.Identity` — user profile + accounts.
    pub identity: IdentityClient<Channel>,
    /// `life.v1.Wallet` — balances + transfer + debit (haima-backed).
    pub wallet: WalletClient<Channel>,
    /// `life.admin.v1.Runtime` — health + session enumeration (operator).
    pub runtime: RuntimeClient<Channel>,
    /// `life.admin.v1.Saga` — saga inspection (operator).
    pub saga: SagaClient<Channel>,
    /// `life.admin.v1.RoutingCache` — route dump + evict (operator).
    pub routing_cache: RoutingCacheClient<Channel>,
}

impl LifedDataloaders {
    /// Construct one set of dataloaders sharing a single gRPC channel.
    /// In production lifegw connects to lifed via UDS (per Spec C₃ §4.2);
    /// in tests the caller passes a tonic-test in-process channel.
    pub fn new(channel: Channel) -> Self {
        Self {
            agent: AgentClient::new(channel.clone()),
            events: EventsClient::new(channel.clone()),
            identity: IdentityClient::new(channel.clone()),
            wallet: WalletClient::new(channel.clone()),
            runtime: RuntimeClient::new(channel.clone()),
            saga: SagaClient::new(channel.clone()),
            routing_cache: RoutingCacheClient::new(channel),
        }
    }

    /// Test-only no-op dataloader for unit tests that never reach
    /// the gRPC layer (e.g., context.rs scope checks).
    ///
    /// Uses `Channel::from_static(...).connect_lazy()` so construction
    /// never blocks on a real connection; the channel only fails on
    /// first RPC use (tests that DO need RPC use the wiremock-style
    /// fixtures in `tests/`).
    #[cfg(test)]
    pub fn test_stub() -> Self {
        let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
        Self::new(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `test_stub` uses `Channel::from_static(...).connect_lazy()` which
    // registers a `TokioTimer` and therefore requires an active Tokio
    // runtime context. All callers (this test + context.rs tests) must
    // be `#[tokio::test]`.
    #[tokio::test]
    async fn test_stub_constructs_without_panic() {
        let _ = LifedDataloaders::test_stub();
    }

    // Real RPC tests live in tests/warp_graphql_t1_golden.rs (Task 11+).
}
