//! Build state persistence.
//!
//! The [`Database`] trait abstracts over storage backends so the concrete
//! engine (e.g. redb) can be swapped at compile time via feature flags.

#[cfg(feature = "redb")]
pub mod redb;

use std::collections::HashMap;

use crate::error::DatabaseError;

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The database backend selected by the active feature flag.
#[cfg(feature = "redb")]
pub type SelectedDatabase = redb::RedbDatabase;

#[cfg(not(any(feature = "redb")))]
compile_error!("At least one database backend must be enabled (e.g. 'redb').");

// ===========================================================================
// Trait
// ===========================================================================

/// Persistent storage for build records.
///
/// Implementors **must** be safe to share across async tasks
/// (`Send + Sync`).
pub trait Database: Send + Sync {
    /// Persist a completed or failed build record.
    ///
    /// # Arguments
    ///
    /// * `record` - The build record to store
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] on I/O or serialization failure.
    fn save_build(
        &self,
        record: &BuildRecord,
    ) -> impl std::future::Future<Output = Result<(), DatabaseError>> + Send;

    /// Retrieve a build record by recipe name and tag.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe file stem (e.g. `"cosmos-gaiad"`)
    /// * `tag` - Git tag / release version
    ///
    /// # Returns
    ///
    /// `Ok(Some(record))` if found, `Ok(None)` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] on I/O or deserialization failure.
    fn get_build(
        &self,
        recipe: &str,
        tag: &str,
    ) -> impl std::future::Future<Output = Result<Option<BuildRecord>, DatabaseError>> + Send;

    /// List all build records for a given recipe.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe file stem
    ///
    /// # Returns
    ///
    /// A vector of build records, possibly empty.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] on I/O failure.
    fn list_builds(
        &self,
        recipe: &str,
    ) -> impl std::future::Future<Output = Result<Vec<BuildRecord>, DatabaseError>> + Send;

    /// Check whether a specific recipe+tag combination has been built.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe file stem
    /// * `tag` - Git tag
    ///
    /// # Returns
    ///
    /// `true` if a record exists (regardless of status).
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] on I/O failure.
    fn is_built(
        &self,
        recipe: &str,
        tag: &str,
    ) -> impl std::future::Future<Output = Result<bool, DatabaseError>> + Send;
}

// ===========================================================================
// Shared types
// ===========================================================================

/// Persistent record of a single build attempt.
#[derive(Debug, Clone)]
pub struct BuildRecord {
    /// Recipe file stem (e.g. `"cosmos-gaiad"`).
    pub recipe_name: String,
    /// Git tag that was built.
    pub tag: String,
    /// Outcome of the build.
    pub status: BuildStatus,
    /// Final Docker image tag (set on success).
    pub image_tag: Option<String>,
    /// ISO-8601 timestamp when the build started.
    pub started_at: String,
    /// ISO-8601 timestamp when the build completed.
    pub completed_at: Option<String>,
    /// Wall-clock duration in seconds.
    pub duration_secs: Option<u64>,
    /// Error message (set on failure).
    pub error: Option<String>,
    /// Flavor selections used for this build.
    pub flavours: HashMap<String, String>,
}

/// Outcome of a build attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// Build is currently executing.
    InProgress,
    /// Build completed successfully.
    Success,
    /// Build failed.
    Failed,
}
