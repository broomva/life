use std::process::Command;

const SERVICE_NAME: &str = "life-agent-os";

/// Store a secret in the system keychain.
/// Returns `true` on success.
pub fn store(account: &str, secret: &str) -> bool {
    if cfg!(target_os = "macos") {
        store_macos(account, secret)
    } else if cfg!(target_os = "linux") {
        store_linux(account, secret)
    } else {
        false
    }
}

/// Read a secret from the system keychain.
pub fn read(account: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        read_macos(account)
    } else if cfg!(target_os = "linux") {
        read_linux(account)
    } else {
        None
    }
}

/// Delete a secret from the system keychain.
/// Returns `true` on success.
pub fn delete(account: &str) -> bool {
    if cfg!(target_os = "macos") {
        delete_macos(account)
    } else if cfg!(target_os = "linux") {
        delete_linux(account)
    } else {
        false
    }
}

/// Check whether the system keychain is available.
pub fn is_available() -> bool {
    if cfg!(target_os = "macos") {
        Command::new("security")
            .arg("help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else if cfg!(target_os = "linux") {
        Command::new("secret-tool")
            .arg("--version")
            .output()
            .map(|_| true)
            .unwrap_or(false)
    } else {
        false
    }
}

// ── macOS (security CLI) ──

fn store_macos(account: &str, secret: &str) -> bool {
    // Delete first to allow updates (add-generic-password fails if entry exists)
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE_NAME, "-a", account])
        .output();

    Command::new("security")
        .args([
            "add-generic-password",
            "-s",
            SERVICE_NAME,
            "-a",
            account,
            "-w",
            secret,
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn read_macos(account: &str) -> Option<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            SERVICE_NAME,
            "-a",
            account,
            "-w",
        ])
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    } else {
        None
    }
}

fn delete_macos(account: &str) -> bool {
    Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE_NAME, "-a", account])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Linux (secret-tool CLI) ──

fn store_linux(account: &str, secret: &str) -> bool {
    Command::new("secret-tool")
        .args([
            "store",
            "--label",
            &format!("{SERVICE_NAME}/{account}"),
            "service",
            SERVICE_NAME,
            "account",
            account,
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(secret.as_bytes())?;
            }
            child.wait()
        })
        .map(|s| s.success())
        .unwrap_or(false)
}

fn read_linux(account: &str) -> Option<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE_NAME, "account", account])
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    } else {
        None
    }
}

fn delete_linux(account: &str) -> bool {
    Command::new("secret-tool")
        .args(["clear", "service", SERVICE_NAME, "account", account])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_does_not_panic() {
        // Just ensure the function runs without panicking on any platform
        let _ = is_available();
    }
}
