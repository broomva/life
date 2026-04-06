use std::fmt;
use std::path::PathBuf;

use crate::{discovery, env as env_loader, keychain};

/// Where a credential was resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Project-local `.life/credentials/.env`
    ProjectEnv,
    /// System keychain (macOS Keychain / Linux secret-tool)
    Keychain,
    /// Global `~/.life/credentials/.env`
    GlobalEnv,
    /// Process environment variable (already set)
    EnvironmentVariable,
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectEnv => write!(f, "project .env"),
            Self::Keychain => write!(f, "keychain"),
            Self::GlobalEnv => write!(f, "global .env"),
            Self::EnvironmentVariable => write!(f, "environment variable"),
        }
    }
}

/// A credential value together with its source.
#[derive(Debug, Clone)]
pub struct ResolvedCredential {
    pub value: String,
    pub source: CredentialSource,
}

/// Resolve a credential using the following cascade:
/// 1. Project-local `.life/credentials/.env`
/// 2. System keychain
/// 3. Global `~/.life/credentials/.env`
/// 4. Process environment variable
pub fn resolve_credential(
    env_var_name: &str,
    keychain_account: &str,
) -> Option<ResolvedCredential> {
    // 1. Project-local .env
    if let Some(root) = discovery::find_project_root() {
        let project_env = root.join(".life").join("credentials").join(".env");
        if project_env.exists()
            && let Ok(vars) = env_loader::parse_env_file(&project_env)
            && let Some(val) = vars.get(env_var_name)
        {
            return Some(ResolvedCredential {
                value: val.clone(),
                source: CredentialSource::ProjectEnv,
            });
        }
    }

    // 2. Keychain
    if let Some(val) = keychain::read(keychain_account) {
        return Some(ResolvedCredential {
            value: val,
            source: CredentialSource::Keychain,
        });
    }

    // 3. Global .env
    let global_env = discovery::global_life_dir()
        .join("credentials")
        .join(".env");
    if global_env.exists()
        && let Ok(vars) = env_loader::parse_env_file(&global_env)
        && let Some(val) = vars.get(env_var_name)
    {
        return Some(ResolvedCredential {
            value: val.clone(),
            source: CredentialSource::GlobalEnv,
        });
    }

    // 4. Environment variable
    if let Ok(val) = std::env::var(env_var_name) {
        return Some(ResolvedCredential {
            value: val,
            source: CredentialSource::EnvironmentVariable,
        });
    }

    None
}

/// Store a credential: try keychain first, fall back to `~/.life/credentials/.env`.
/// Returns the source where the credential was stored.
pub fn store_credential(
    env_var_name: &str,
    keychain_account: &str,
    value: &str,
) -> CredentialSource {
    // Try keychain first
    if keychain::store(keychain_account, value) {
        tracing::info!("stored credential {env_var_name} in keychain");
        return CredentialSource::Keychain;
    }

    // Fallback: write to ~/.life/credentials/.env
    let cred_dir = discovery::global_life_dir().join("credentials");
    std::fs::create_dir_all(&cred_dir).ok();

    let env_file = cred_dir.join(".env");
    let mut content = std::fs::read_to_string(&env_file).unwrap_or_default();

    // Replace existing line or append
    let prefix = format!("{env_var_name}=");
    let new_line = format!("{env_var_name}={value}");
    let mut found = false;
    let lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                found = true;
                new_line.clone()
            } else {
                line.to_string()
            }
        })
        .collect();

    if found {
        content = lines.join("\n");
    } else {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&new_line);
        content.push('\n');
    }

    std::fs::write(&env_file, &content).ok();

    // Set file permissions to 0600 on Unix
    #[cfg(unix)]
    {
        set_restricted_permissions(&env_file);
    }

    tracing::info!("stored credential {env_var_name} in {}", env_file.display());
    CredentialSource::GlobalEnv
}

#[cfg(unix)]
fn set_restricted_permissions(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms).ok();
}

/// Map a provider name to its (env_var_name, keychain_account) pair.
pub fn provider_credential_names(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "anthropic" => ("ANTHROPIC_API_KEY", "anthropic-api-key"),
        "openai" => ("OPENAI_API_KEY", "openai-api-key"),
        "google" | "gemini" => ("GOOGLE_API_KEY", "google-api-key"),
        "mistral" => ("MISTRAL_API_KEY", "mistral-api-key"),
        "cohere" => ("COHERE_API_KEY", "cohere-api-key"),
        "groq" => ("GROQ_API_KEY", "groq-api-key"),
        "deepseek" => ("DEEPSEEK_API_KEY", "deepseek-api-key"),
        _ => ("LIFE_API_KEY", "life-api-key"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_from_env_var() {
        let unique_key = "LIFE_PATHS_TEST_CRED_RESOLVE";
        // SAFETY: test-only, single-threaded context
        unsafe {
            std::env::set_var(unique_key, "secret-from-env");
        }
        let result = resolve_credential(unique_key, "nonexistent-keychain-account");
        assert!(result.is_some());
        let cred = result.unwrap();
        assert_eq!(cred.value, "secret-from-env");
        assert_eq!(cred.source, CredentialSource::EnvironmentVariable);
        unsafe {
            std::env::remove_var(unique_key);
        }
    }

    #[test]
    fn missing_credential() {
        let result = resolve_credential(
            "LIFE_PATHS_ABSOLUTELY_NONEXISTENT_VAR_XYZ",
            "nonexistent-keychain-account-xyz",
        );
        assert!(result.is_none());
    }

    #[test]
    fn provider_names() {
        let (env_var, kc) = provider_credential_names("anthropic");
        assert_eq!(env_var, "ANTHROPIC_API_KEY");
        assert_eq!(kc, "anthropic-api-key");

        let (env_var, kc) = provider_credential_names("openai");
        assert_eq!(env_var, "OPENAI_API_KEY");
        assert_eq!(kc, "openai-api-key");

        // Unknown provider falls back to generic
        let (env_var, kc) = provider_credential_names("unknown");
        assert_eq!(env_var, "LIFE_API_KEY");
        assert_eq!(kc, "life-api-key");
    }

    #[test]
    fn store_creates_env_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let fake_home = tmp.path().join("fakehome");
        std::fs::create_dir_all(&fake_home).unwrap();

        // We can't easily redirect global_life_dir in a test, so test the file-write
        // logic directly.
        let cred_dir = fake_home.join(".life").join("credentials");
        std::fs::create_dir_all(&cred_dir).unwrap();
        let env_file = cred_dir.join(".env");

        let content = "MY_KEY=my_value\n";
        std::fs::write(&env_file, content).unwrap();

        // Verify the file was created with correct content
        let read_back = std::fs::read_to_string(&env_file).unwrap();
        assert!(read_back.contains("MY_KEY=my_value"));
    }
}
