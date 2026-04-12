//! Build status notification.
//!
//! The [`Notifier`] trait abstracts over notification backends so the
//! concrete provider (e.g. Telegram) can be swapped at compile time.

#[cfg(feature = "telegram")]
pub mod telegram;

use std::time::Duration;

use crate::error::NotifierError;

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The notifier backend selected by the active feature flag.
#[cfg(feature = "telegram")]
pub type SelectedNotifier = telegram::TelegramNotifier;

#[cfg(not(any(feature = "telegram")))]
compile_error!(
    "At least one notification backend must be enabled \
     (e.g. 'telegram')."
);

// ===========================================================================
// Trait
// ===========================================================================

/// Sends build lifecycle notifications to an external channel.
///
/// Notification delivery is best-effort: failures are logged but should
/// not abort builds.
pub trait Notifier: Send + Sync {
    /// Notify that a build has started.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name (e.g. `"Cosmos"`)
    /// * `tag` - Git tag being built
    ///
    /// # Errors
    ///
    /// Returns [`NotifierError`] if delivery fails.
    fn notify_build_start(
        &self,
        recipe: &str,
        tag: &str,
    ) -> impl std::future::Future<Output = Result<(), NotifierError>> + Send;

    /// Notify that a build succeeded.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name
    /// * `tag` - Git tag
    /// * `duration` - Wall-clock build duration
    ///
    /// # Errors
    ///
    /// Returns [`NotifierError`] if delivery fails.
    fn notify_build_success(
        &self,
        recipe: &str,
        tag: &str,
        duration: Duration,
    ) -> impl std::future::Future<Output = Result<(), NotifierError>> + Send;

    /// Notify that a build failed.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name
    /// * `tag` - Git tag
    /// * `error` - Human-readable error description
    ///
    /// # Errors
    ///
    /// Returns [`NotifierError`] if delivery fails.
    fn notify_build_failure(
        &self,
        recipe: &str,
        tag: &str,
        error: &str,
    ) -> impl std::future::Future<Output = Result<(), NotifierError>> + Send;
}
