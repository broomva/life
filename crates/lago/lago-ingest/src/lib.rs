pub mod client;
pub mod codec;
pub mod server;

/// Generated protobuf types.
#[allow(unused_qualifications, clippy::all)]
pub mod proto {
    tonic::include_proto!("lago.v1");
}

pub use client::{IngestClient, IngestReceiver, IngestSender};
pub use server::IngestServer;
