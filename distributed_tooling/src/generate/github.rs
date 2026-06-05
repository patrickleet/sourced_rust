//! GitHub repository parsing (and, in a follow-up slice, the GitHub Actions
//! workflow + GitOps promotion-chart templates).

use crate::{GithubRepo, ScaffoldError};

/// Parse an `owner/repo` string, validating both halves.
pub(crate) fn parse_github_repo(raw: &str) -> Result<GithubRepo, ScaffoldError> {
    let trimmed = raw.trim();
    let Some((owner, repo)) = trimmed.split_once('/') else {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    }
    let valid = [owner, repo].into_iter().all(|part| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    });
    if !valid {
        return Err(ScaffoldError::new(
            "repository contains unsupported GitHub characters",
        ));
    }
    Ok(GithubRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}
