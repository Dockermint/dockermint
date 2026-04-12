//! Container registry authentication and image pushing.
//!
//! The [`RegistryClient`] trait abstracts over registry protocols so the
//! concrete backend (e.g. OCI) can be swapped at compile time.

#[cfg(feature = "oci")]
pub mod oci;

use crate::error::RegistryError;

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The registry backend selected by the active feature flag.
#[cfg(feature = "oci")]
pub type SelectedRegistry = oci::OciRegistry;

#[cfg(not(any(feature = "oci")))]
compile_error!("At least one registry backend must be enabled (e.g. 'oci').");

// ===========================================================================
// Trait
// ===========================================================================

/// Pushes built images to a container registry.
pub trait RegistryClient: Send + Sync {
    /// Authenticate with the registry.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Auth`] on credential rejection.
    fn authenticate(&self) -> impl std::future::Future<Output = Result<(), RegistryError>> + Send;

    /// Push a local image to the registry.
    ///
    /// # Arguments
    ///
    /// * `image` - Full image reference (e.g.
    ///   `"ghcr.io/dockermint/cosmos-gaiad"`)
    /// * `tag` - Tag to push
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Push`] on failure.
    fn push_image(
        &self,
        image: &str,
        tag: &str,
    ) -> impl std::future::Future<Output = Result<(), RegistryError>> + Send;

    /// Check whether a tag already exists in the registry.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference
    /// * `tag` - Tag to check
    ///
    /// # Returns
    ///
    /// `true` if the tag is present.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Query`] on network failure.
    fn tag_exists(
        &self,
        image: &str,
        tag: &str,
    ) -> impl std::future::Future<Output = Result<bool, RegistryError>> + Send;
}
