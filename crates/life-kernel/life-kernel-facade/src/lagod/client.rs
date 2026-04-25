//! Reqwest wrapper around lagod's HTTP surface.

use crate::{
    config::DaemonEndpoints,
    error::{FacadeError, FacadeResult},
};
use reqwest::{Client, Url};

/// Shared reqwest client for every lagod proxy (Events, Knowledge,
/// Blobs, Billing — only Events wired in Phase 1).
#[derive(Clone)]
pub struct LagoClient {
    inner: Client,
    base: Url,
    bearer: Option<String>,
}

impl LagoClient {
    /// Construct from daemon endpoint configuration.
    pub fn new(endpoints: &DaemonEndpoints) -> FacadeResult<Self> {
        let base = endpoints
            .lagod
            .parse::<Url>()
            .map_err(|e| FacadeError::BackendProtocol {
                daemon: "lagod",
                reason: format!("bad base url: {e}"),
            })?;
        let inner = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| FacadeError::BackendUnavailable {
                daemon: "lagod",
                source: e.into(),
            })?;
        Ok(Self {
            inner,
            base,
            bearer: endpoints.bearer_token.clone(),
        })
    }

    pub(crate) fn url(&self, path: &str) -> Url {
        self.base
            .join(path.trim_start_matches('/'))
            .expect("valid join")
    }

    pub(crate) fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.inner.request(method, self.url(path));
        if let Some(ref token) = self.bearer {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Expose the inner reqwest client for stream-body consumers (SSE).
    #[allow(dead_code)] // Reserved for direct-stream consumers added in Phase 2.
    pub(crate) fn raw(&self) -> &Client {
        &self.inner
    }
}
