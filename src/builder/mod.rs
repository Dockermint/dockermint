//! Docker image building: Dockerfile generation, template expansion,
//! and buildx orchestration.
//!
//! The [`ImageBuilder`] trait abstracts over builder backends so the
//! concrete engine (e.g. BuildKit) can be swapped at compile time.
//!
//! # Sub-modules
//!
//! - [`template`](crate::builder::template) -- `{{variable}}` interpolation engine
//! - [`go`](crate::builder::go) -- Go-specific build command generation

pub mod dockerfile;
pub mod template;

#[cfg(feature = "buildkit")]
pub mod buildkit;

pub mod go;

use std::collections::HashMap;
use std::time::Duration;

use crate::error::BuilderError;
use crate::recipe::types::{ResolvedRecipe, SelectedFlavours};

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The builder backend selected by the active feature flag.
#[cfg(feature = "buildkit")]
pub type SelectedBuilder = buildkit::BuildKitBuilder;

#[cfg(not(any(feature = "buildkit")))]
compile_error!("At least one builder backend must be enabled (e.g. 'buildkit').");

// ===========================================================================
// Trait
// ===========================================================================

/// Builds Docker images from resolved recipes.
pub trait ImageBuilder: Send + Sync {
    /// Set up platform-specific builder instances.
    ///
    /// For BuildKit this creates `dockermint-amd64` and
    /// `dockermint-arm64` buildx builders.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError::BuildxSetup`] if builder creation fails.
    fn setup_builders(&self) -> impl std::future::Future<Output = Result<(), BuilderError>> + Send;

    /// Build a Docker image from a resolved recipe.
    ///
    /// # Arguments
    ///
    /// * `context` - All information needed for the build
    ///
    /// # Returns
    ///
    /// [`BuildOutput`] containing the image ID, tag, and duration.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] on Dockerfile generation, template
    /// expansion, or build command failure.
    fn build(
        &self,
        context: &BuildContext,
    ) -> impl std::future::Future<Output = Result<BuildOutput, BuilderError>> + Send;

    /// Remove temporary build artifacts and builder instances.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] if cleanup commands fail.
    fn cleanup(&self) -> impl std::future::Future<Output = Result<(), BuilderError>> + Send;
}

// ===========================================================================
// Build types
// ===========================================================================

/// Everything needed to execute a single build.
#[derive(Debug, Clone)]
pub struct BuildContext {
    /// Resolved recipe with concrete flavor selections.
    pub recipe: ResolvedRecipe,

    /// Git tag being built.
    pub tag: String,

    /// Fully resolved template variables (host + build).
    pub variables: HashMap<String, String>,

    /// Target platforms (e.g. `["linux/amd64", "linux/arm64"]`).
    pub platforms: Vec<String>,
}

/// Result of a successful build.
#[derive(Debug, Clone)]
pub struct BuildOutput {
    /// Docker image ID.
    pub image_id: String,

    /// Full image tag (e.g.
    /// `"cosmos-gaiad-goleveldb:v21.0.1-alpine3.23"`).
    pub image_tag: String,

    /// Wall-clock build duration.
    pub duration: Duration,

    /// Platforms that were built.
    pub platforms: Vec<String>,
}

impl BuildContext {
    /// Create a new build context.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Resolved recipe
    /// * `tag` - Git tag to build
    /// * `platforms` - Target platforms
    pub fn new(recipe: ResolvedRecipe, tag: String, platforms: Vec<String>) -> Self {
        let variables = recipe.resolved_variables.clone();
        Self {
            recipe,
            tag,
            variables,
            platforms,
        }
    }

    /// Resolve the image tag template using current variables.
    ///
    /// # Returns
    ///
    /// The expanded image tag string.
    pub fn resolve_image_tag(&self) -> String {
        template::TemplateEngine::render(&self.recipe.recipe.image.tag, &self.variables)
    }

    /// Get the selected flavours for this build context.
    ///
    /// # Returns
    ///
    /// A reference to the [`SelectedFlavours`] resolved from CLI args,
    /// config, or recipe defaults.
    pub fn flavor(&self) -> &SelectedFlavours {
        &self.recipe.selected_flavours
    }
}
