use std::collections::HashMap;
use std::io;
use std::path::Path;

/// Parse a `.env` file into key-value pairs.
pub fn parse_env_file(path: &Path) -> io::Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)?;
    Ok(parse_env_str(&content))
}

/// Parse `.env`-style content: `KEY=VALUE`, with optional quotes and comments.
pub fn parse_env_str(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let mut value = value.trim().to_string();
            // Strip surrounding quotes (single or double)
            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value = value[1..value.len() - 1].to_string();
            }
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

/// Load environment variables from a `.env` file.
/// Only sets variables that are not already present in the environment.
pub fn load_env(path: &Path) {
    let vars = match parse_env_file(path) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("could not load env file {}: {}", path.display(), e);
            return;
        }
    };
    for (key, value) in vars {
        if std::env::var_os(&key).is_none() {
            // SAFETY: Rust 2024 edition marks set_var as unsafe because it is not
            // thread-safe. We accept this risk during initialization.
            unsafe {
                std::env::set_var(&key, &value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_key_value() {
        let content = "FOO=bar\nBAZ=123";
        let map = parse_env_str(content);
        assert_eq!(map.get("FOO").unwrap(), "bar");
        assert_eq!(map.get("BAZ").unwrap(), "123");
    }

    #[test]
    fn quoted_values() {
        let content = "A=\"hello world\"\nB='single quoted'";
        let map = parse_env_str(content);
        assert_eq!(map.get("A").unwrap(), "hello world");
        assert_eq!(map.get("B").unwrap(), "single quoted");
    }

    #[test]
    fn comments_and_empty_lines() {
        let content = "# this is a comment\n\nKEY=value\n  # indented comment\n";
        let map = parse_env_str(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("KEY").unwrap(), "value");
    }

    #[test]
    fn file_not_found() {
        let result = parse_env_file(Path::new("/nonexistent/.env"));
        assert!(result.is_err());
    }
}
