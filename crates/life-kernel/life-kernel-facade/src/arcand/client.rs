//! Reqwest wrapper around arcand's HTTP surface.

use crate::{
    config::DaemonEndpoints,
    error::{FacadeError, FacadeResult},
};
use reqwest::{Client, Url};

/// Shared reqwest client for every arcand proxy (Session, Approvals —
/// wired in Phase 1).
#[derive(Clone)]
pub struct ArcanClient {
    inner: Client,
    base: Url,
    bearer: Option<String>,
}

impl ArcanClient {
    /// Construct from daemon endpoint configuration.
    pub fn new(endpoints: &DaemonEndpoints) -> FacadeResult<Self> {
        let base = endpoints
            .arcand
            .parse::<Url>()
            .map_err(|e| FacadeError::BackendProtocol {
                daemon: "arcand",
                reason: format!("bad base url: {e}"),
            })?;
        let inner = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| FacadeError::BackendUnavailable {
                daemon: "arcand",
                source: e.into(),
            })?;
        Ok(Self {
            inner,
            base,
            bearer: endpoints.bearer_token.clone(),
        })
    }

    pub(crate) fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .expect("valid join");
        let mut req = self.inner.request(method, url);
        if let Some(ref token) = self.bearer {
            req = req.bearer_auth(token);
        }
        req
    }
}
