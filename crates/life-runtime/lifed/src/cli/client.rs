//! Shared admin-plane tonic client builder. Sub-phase A scaffolds; C5 fills in.

use std::path::Path;

use anyhow::Result;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

pub async fn connect(socket: &Path) -> Result<Channel> {
    let socket = socket.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::]:0")?;
    let channel = endpoint
        .connect_with_connector(service_fn(move |_: Uri| {
            let socket = socket.clone();
            async move {
                let s = UnixStream::connect(socket).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
            }
        }))
        .await?;
    Ok(channel)
}
