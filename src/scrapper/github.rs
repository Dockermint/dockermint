//! GitHub VCS backend using the REST API via [`reqwest`].
//!
//! Fetches releases from a GitHub repository and filters them by
//! include/exclude glob patterns defined in the recipe header.

use globset::{Glob, GlobSet, GlobSetBuilder};
use secrecy::{ExposeSecret, SecretString};

use crate::error::VcsError;
use crate::scrapper::{Release, TagFilter, VersionControlSystem};

/// GitHub REST API client for fetching releases.
#[derive(Debug)]
pub struct GithubClient {
    client: reqwest::Client,
    /// Optional basic-auth credentials (user, PAT).
    auth: Option<(String, SecretString)>,
}

impl GithubClient {
    /// Create a new GitHub client.
    ///
    /// # Arguments
    ///
    /// * `user` - GitHub username (optional, for authenticated requests)
    /// * `pat` - Personal access token (optional)
    ///
    /// # Errors
    ///
    /// Returns [`VcsError::Auth`] if only one of user/pat is provided.
    pub fn new(user: Option<&str>, pat: Option<&str>) -> Result<Self, VcsError> {
        match (user, pat) {
            (Some(u), Some(p)) => Ok(Self {
                client: reqwest::Client::new(),
                auth: Some((u.to_owned(), SecretString::from(p.to_owned()))),
            }),
            (None, None) => Ok(Self {
                client: reqwest::Client::new(),
                auth: None,
            }),
            _ => Err(VcsError::Auth(
                "both GH_USER and GH_PAT must be set, or neither".to_owned(),
            )),
        }
    }

    /// Extract `owner/repo` from a full GitHub URL.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Full URL (e.g. `https://github.com/cosmos/gaia`)
    ///
    /// # Returns
    ///
    /// `"owner/repo"` string.
    fn parse_owner_repo(repo_url: &str) -> Result<String, VcsError> {
        let stripped = repo_url.trim_end_matches('/').trim_end_matches(".git");
        let parts: Vec<&str> = stripped.rsplitn(3, '/').collect();
        if parts.len() < 2 {
            return Err(VcsError::Parse(format!(
                "cannot extract owner/repo from: {repo_url}"
            )));
        }
        Ok(format!("{}/{}", parts[1], parts[0]))
    }

    /// Fetch a single page of releases from the GitHub API.
    async fn fetch_page(
        &self,
        owner_repo: &str,
        page: u32,
    ) -> Result<Vec<GithubRelease>, VcsError> {
        let url =
            format!("https://api.github.com/repos/{owner_repo}/releases?per_page=100&page={page}");

        let mut req = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "dockermint");

        if let Some((user, pat)) = &self.auth {
            req = req.basic_auth(user, Some(pat.expose_secret()));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| VcsError::Request(format!("{url}: {e}")))?;

        // Handle rate limiting
        if resp.status() == reqwest::StatusCode::FORBIDDEN
            || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            let retry = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(VcsError::RateLimit {
                retry_after_secs: retry,
            });
        }

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(VcsError::Auth(
                "GitHub API returned 401 -- check GH_USER/GH_PAT".to_owned(),
            ));
        }

        if !resp.status().is_success() {
            return Err(VcsError::Request(format!("{url}: HTTP {}", resp.status())));
        }

        resp.json::<Vec<GithubRelease>>()
            .await
            .map_err(|e| VcsError::Parse(format!("JSON: {e}")))
    }
}

impl VersionControlSystem for GithubClient {
    /// Fetch releases, paginating through all pages, and apply tag
    /// filters.
    ///
    /// Results are returned newest-first.
    async fn fetch_releases(
        &self,
        repo_url: &str,
        filter: &TagFilter,
    ) -> Result<Vec<Release>, VcsError> {
        let owner_repo = Self::parse_owner_repo(repo_url)?;

        let include_set = build_glob_set(&filter.include_patterns)?;
        let exclude_set = build_glob_set(&filter.exclude_patterns)?;

        let mut all_releases = Vec::new();
        let mut page = 1u32;

        loop {
            let releases = self.fetch_page(&owner_repo, page).await?;
            let count = releases.len();

            for gh in releases {
                let tag = gh.tag_name;

                // Apply include filter (if non-empty, tag must match)
                if let Some(ref set) = include_set
                    && !set.is_match(&tag)
                {
                    continue;
                }

                // Apply exclude filter
                if let Some(ref set) = exclude_set
                    && set.is_match(&tag)
                {
                    continue;
                }

                all_releases.push(Release {
                    tag,
                    prerelease: gh.prerelease,
                    published_at: gh.published_at,
                });
            }

            // GitHub returns up to 100 per page; fewer means last page
            if count < 100 {
                break;
            }
            page += 1;
        }

        Ok(all_releases)
    }
}

// ── helpers ──────────────────────────────────────────────────────────

/// Build a [`GlobSet`] from a comma-separated pattern string.
///
/// Returns `None` if the input is empty.
fn build_glob_set(patterns: &str) -> Result<Option<GlobSet>, VcsError> {
    let trimmed = patterns.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in trimmed.split(',') {
        let p = pattern.trim();
        if !p.is_empty() {
            let glob =
                Glob::new(p).map_err(|e| VcsError::Parse(format!("invalid glob '{p}': {e}")))?;
            builder.add(glob);
        }
    }

    let set = builder
        .build()
        .map_err(|e| VcsError::Parse(format!("glob set: {e}")))?;
    Ok(Some(set))
}

/// Subset of the GitHub Release API response we care about.
#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_owner_repo_https() {
        let result =
            GithubClient::parse_owner_repo("https://github.com/cosmos/gaia").expect("parse");
        assert_eq!(result, "cosmos/gaia");
    }

    #[test]
    fn parse_owner_repo_trailing_slash() {
        let result =
            GithubClient::parse_owner_repo("https://github.com/KYVENetwork/chain/").expect("parse");
        assert_eq!(result, "KYVENetwork/chain");
    }

    #[test]
    fn parse_owner_repo_dot_git() {
        let result =
            GithubClient::parse_owner_repo("https://github.com/cosmos/gaia.git").expect("parse");
        assert_eq!(result, "cosmos/gaia");
    }

    #[test]
    fn build_glob_set_empty() {
        assert!(build_glob_set("").expect("ok").is_none());
        assert!(build_glob_set("  ").expect("ok").is_none());
    }

    #[test]
    fn build_glob_set_single() {
        let set = build_glob_set("v*").expect("ok").expect("some");
        assert!(set.is_match("v21.0.1"));
        assert!(!set.is_match("release-1"));
    }

    #[test]
    fn build_glob_set_multiple() {
        let set = build_glob_set("v*, release-*").expect("ok").expect("some");
        assert!(set.is_match("v1.0"));
        assert!(set.is_match("release-2"));
        assert!(!set.is_match("nightly"));
    }

    #[test]
    fn github_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GithubClient>();
    }

    #[test]
    fn new_no_auth() {
        let client = GithubClient::new(None, None).expect("ok");
        assert!(client.auth.is_none());
    }

    #[test]
    fn new_partial_auth_fails() {
        let err = GithubClient::new(Some("user"), None).unwrap_err();
        assert!(matches!(err, VcsError::Auth(_)));
    }

    // -- additional tests for mutation coverage --

    #[test]
    fn new_full_auth_stores_credentials() {
        let client = GithubClient::new(Some("myuser"), Some("mytoken")).expect("ok");
        let (user, pat) = client.auth.as_ref().expect("auth should be Some");
        assert_eq!(user, "myuser");
        assert_eq!(pat.expose_secret(), "mytoken");
    }

    #[test]
    fn new_pat_only_fails() {
        let err = GithubClient::new(None, Some("token")).unwrap_err();
        assert!(matches!(err, VcsError::Auth(_)));
    }

    #[test]
    fn new_partial_auth_error_message_content() {
        let err = GithubClient::new(Some("user"), None).unwrap_err();
        match err {
            VcsError::Auth(msg) => {
                assert!(
                    msg.contains("GH_USER") && msg.contains("GH_PAT"),
                    "error should mention GH_USER and GH_PAT, got: {msg}"
                );
            },
            other => panic!("expected VcsError::Auth, got: {other:?}"),
        }
    }

    #[test]
    fn parse_owner_repo_trailing_dot_git_and_slash() {
        let result =
            GithubClient::parse_owner_repo("https://github.com/cosmos/gaia.git/").expect("parse");
        assert_eq!(result, "cosmos/gaia");
    }

    #[test]
    fn parse_owner_repo_bare_path_fails() {
        let err = GithubClient::parse_owner_repo("cosmos").expect_err("should fail");
        assert!(matches!(err, VcsError::Parse(_)));
    }

    #[test]
    fn parse_owner_repo_empty_string_fails() {
        let err = GithubClient::parse_owner_repo("").expect_err("should fail");
        assert!(matches!(err, VcsError::Parse(_)));
    }

    #[test]
    fn parse_owner_repo_ssh_style() {
        // rsplitn(3, '/') handles any prefix with at least two slashes
        let result = GithubClient::parse_owner_repo("git@github.com/cosmos/gaia").expect("parse");
        assert_eq!(result, "cosmos/gaia");
    }

    #[test]
    fn build_glob_set_trailing_comma() {
        let set = build_glob_set("v*,").expect("ok").expect("some");
        assert!(set.is_match("v1.0"));
    }

    #[test]
    fn build_glob_set_leading_comma() {
        let set = build_glob_set(",v*").expect("ok").expect("some");
        assert!(set.is_match("v1.0"));
    }

    #[test]
    fn build_glob_set_only_commas_produces_empty_set() {
        // All entries are empty after trim, so no globs are added.
        // The builder still produces a GlobSet (Some), but it matches
        // nothing.
        let set = build_glob_set(",,, ,").expect("ok").expect("some");
        assert!(
            !set.is_match("anything"),
            "an empty glob set should not match any input"
        );
    }

    #[test]
    fn build_glob_set_exact_match() {
        let set = build_glob_set("v1.0.0").expect("ok").expect("some");
        assert!(set.is_match("v1.0.0"));
        assert!(!set.is_match("v1.0.1"));
    }

    #[test]
    fn build_glob_set_question_mark_wildcard() {
        let set = build_glob_set("v1.?.0").expect("ok").expect("some");
        assert!(set.is_match("v1.0.0"));
        assert!(set.is_match("v1.9.0"));
        assert!(!set.is_match("v1.10.0"));
    }

    #[test]
    fn github_release_deserializes_minimal() {
        let json = r#"{"tag_name": "v1.0.0"}"#;
        let release: GithubRelease = serde_json::from_str(json).expect("parse");
        assert_eq!(release.tag_name, "v1.0.0");
        assert!(!release.prerelease);
        assert!(release.published_at.is_none());
    }

    #[test]
    fn github_release_deserializes_full() {
        let json = r#"{
            "tag_name": "v2.0.0",
            "prerelease": true,
            "published_at": "2026-01-01T00:00:00Z"
        }"#;
        let release: GithubRelease = serde_json::from_str(json).expect("parse");
        assert_eq!(release.tag_name, "v2.0.0");
        assert!(release.prerelease);
        assert_eq!(
            release.published_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn github_release_prerelease_defaults_false() {
        let json = r#"{"tag_name": "v3.0.0", "published_at": null}"#;
        let release: GithubRelease = serde_json::from_str(json).expect("parse");
        assert!(!release.prerelease);
    }

    #[test]
    fn build_glob_set_invalid_pattern_returns_error() {
        let result = build_glob_set("[invalid");
        assert!(result.is_err());
    }
}
