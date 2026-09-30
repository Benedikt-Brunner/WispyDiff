use std::process::Command;

use crate::error::{Error, Result};

/// `WISPY_GITHUB_TOKEN` wins (used by tests); otherwise reuse the `gh` CLI login.
pub fn resolve_github_token() -> Result<String> {
    if let Ok(token) = std::env::var("WISPY_GITHUB_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }
    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .output()
        .map_err(|e| Error::NoToken(format!("could not run `gh auth token` ({e}); is the GitHub CLI installed?")))?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || token.is_empty() {
        return Err(Error::NoToken(format!(
            "`gh auth token` failed: {}. Run `gh auth login` first.",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(token)
}
