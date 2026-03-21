//! HTTP API for Nous evaluation endpoints.

pub mod routes;
pub mod store;

pub use routes::nous_router;
pub use store::ScoreStore;
