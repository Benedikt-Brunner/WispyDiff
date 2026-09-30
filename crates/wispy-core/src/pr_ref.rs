use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Identifies a pull request: `owner/repo#number`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    /// Accepts `https://github.com/o/r/pull/12[/files…]`, `github.com/o/r/pull/12`,
    /// `o/r/pull/12` and `o/r#12`.
    pub fn parse(input: &str) -> Result<Self> {
        let invalid = || Error::InvalidPrRef(input.to_string());
        let trimmed = input.trim();
        let without_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);
        let without_host = without_scheme
            .strip_prefix("www.github.com/")
            .or_else(|| without_scheme.strip_prefix("github.com/"))
            .unwrap_or(without_scheme);
        let (owner, repo, number) = match without_host.split_once('#') {
            Some((repo_path, number)) if !repo_path.contains("/pull/") => {
                let (owner, repo) = repo_path.split_once('/').ok_or_else(invalid)?;
                (owner, repo, number)
            }
            _ => {
                let path = without_host.split(['?', '#']).next().unwrap_or(without_host);
                Self::split_pull_path(path).ok_or_else(invalid)?
            }
        };

        let number: u64 = number.parse().map_err(|_| invalid())?;
        if number == 0 || !is_valid_name(owner) || !is_valid_name(repo) {
            return Err(invalid());
        }
        Ok(PrRef { owner: owner.to_string(), repo: repo.to_string(), number })
    }

    fn split_pull_path(path: &str) -> Option<(&str, &str, &str)> {
        let mut parts = path.trim_end_matches('/').split('/');
        let owner = parts.next()?;
        let repo = parts.next()?;
        if parts.next()? != "pull" {
            return None;
        }
        let number = parts.next()?;
        Some((owner, repo, number))
    }

    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl std::fmt::Display for PrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
