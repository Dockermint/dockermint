//! Version control system integration for fetching releases and tags.
//!
//! The [`VersionControlSystem`](crate::scrapper::VersionControlSystem) trait
//! abstracts over VCS providers so the concrete backend (e.g. GitHub) can be
//! swapped at compile time.

#[cfg(feature = "github")]
pub mod github;

use crate::error::VcsError;

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The VCS backend selected by the active feature flag.
#[cfg(feature = "github")]
pub type SelectedVcs = github::GithubClient;

#[cfg(not(any(feature = "github")))]
compile_error!("At least one VCS backend must be enabled (e.g. 'github').");

// ===========================================================================
// Trait
// ===========================================================================

/// Fetches releases/tags from a version control system.
pub trait VersionControlSystem: Send + Sync {
    /// Fetch releases from the repository, applying tag filters.
    ///
    /// # Arguments
    ///
    /// * `repo_url` - Full repository URL (e.g.
    ///   `"https://github.com/cosmos/gaia"`)
    /// * `filter` - Include/exclude glob patterns
    ///
    /// # Returns
    ///
    /// Releases sorted newest-first.
    ///
    /// # Errors
    ///
    /// Returns [`VcsError`] on network, parse, or auth failure.
    fn fetch_releases(
        &self,
        repo_url: &str,
        filter: &TagFilter,
    ) -> impl std::future::Future<Output = Result<Vec<Release>, VcsError>> + Send;
}

// ===========================================================================
// Shared types
// ===========================================================================

/// A release (or tag) fetched from a VCS provider.
#[derive(Debug, Clone)]
pub struct Release {
    /// Tag name (e.g. `"v21.0.1"`).
    pub tag: String,

    /// Whether this is marked as a pre-release.
    pub prerelease: bool,

    /// ISO-8601 publication timestamp.
    pub published_at: Option<String>,
}

/// Glob-based include/exclude filter for tags.
#[derive(Debug, Clone, Default)]
pub struct TagFilter {
    /// Comma-separated glob patterns.  If non-empty, only matching tags
    /// pass.
    pub include_patterns: String,

    /// Comma-separated glob patterns.  Matching tags are excluded even
    /// if they match an include pattern.
    pub exclude_patterns: String,
}
