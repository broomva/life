//! Daemon endpoint configuration.

use serde::{Deserialize, Serialize};

/// Where the facade reaches each downstream daemon. Typically sourced
/// from `lifed`'s `/etc/lifed/config.toml` `[daemons]` section; tests
/// construct this directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DaemonEndpoints {
    /// HTTP base URL for arcand (e.g. `http://localhost:3000`).
    pub arcand: String,
    /// HTTP base URL for lagod (e.g. `http://localhost:3001`).
    pub lagod: String,
    /// Optional bearer token for authenticated daemon calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
}

impl DaemonEndpoints {
    /// Builder used in tests — caller supplies both URLs explicitly.
    pub fn new(arcand: impl Into<String>, lagod: impl Into<String>) -> Self {
        Self {
            arcand: arcand.into(),
            lagod: lagod.into(),
            bearer_token: None,
        }
    }
}
