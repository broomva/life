//! Feed configuration loader — parses `feeds.toml` into [`FeedsConfig`].

use std::path::Path;

use opsis_core::feed::FeedsConfig;

use crate::error::{EngineError, EngineResult};

/// Load a [`FeedsConfig`] from a TOML file at the given path.
pub fn load_feeds_config(path: &Path) -> EngineResult<FeedsConfig> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| EngineError::Config(format!("failed to read {}: {e}", path.display())))?;
    let config: FeedsConfig = toml::from_str(&contents)
        .map_err(|e| EngineError::Config(format!("failed to parse {}: {e}", path.display())))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn load_valid_feeds_toml() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"
[[feeds]]
name = "usgs-earthquake"
connector = "poll"
url = "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_hour.geojson"
interval_secs = 30
schema = "usgs.geojson.v1"
domain = "Emergency"

[[feeds]]
name = "open-meteo"
connector = "poll"
url = "https://api.open-meteo.com/v1/forecast"
interval_secs = 300
schema = "openmeteo.current.v1"
domain = "Weather"
"#
        )
        .unwrap();

        let config = load_feeds_config(f.path()).unwrap();
        assert_eq!(config.feeds.len(), 2);
        assert_eq!(config.feeds[0].name, "usgs-earthquake");
        assert_eq!(config.feeds[1].name, "open-meteo");
    }

    #[test]
    fn load_missing_file_returns_error() {
        let result = load_feeds_config(Path::new("/nonexistent/feeds.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_invalid_toml_returns_error() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not valid toml [[[").unwrap();
        let result = load_feeds_config(f.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_empty_feeds() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "").unwrap();
        let config = load_feeds_config(f.path()).unwrap();
        assert!(config.feeds.is_empty());
    }
}
