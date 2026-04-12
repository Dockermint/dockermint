//! Build metrics collection and exposition.
//!
//! The [`MetricsCollector`](crate::metrics::MetricsCollector) trait abstracts
//! over metrics backends so the concrete system (e.g. Prometheus) can be
//! swapped at compile time.

#[cfg(feature = "prometheus")]
pub mod prometheus;

use std::net::SocketAddr;
use std::time::Duration;

use crate::error::MetricsError;

// ===========================================================================
// Compile-time backend selection
// ===========================================================================

/// The metrics backend selected by the active feature flag.
#[cfg(feature = "prometheus")]
pub type SelectedMetrics = prometheus::PrometheusCollector;

#[cfg(not(any(feature = "prometheus")))]
compile_error!(
    "At least one metrics backend must be enabled \
     (e.g. 'prometheus')."
);

// ===========================================================================
// Trait
// ===========================================================================

/// Collects build metrics and exposes them via an HTTP endpoint.
pub trait MetricsCollector: Send + Sync {
    /// Record that a build has started.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name
    /// * `tag` - Git tag being built
    fn record_build_start(&self, recipe: &str, tag: &str);

    /// Record that a build succeeded.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name
    /// * `tag` - Git tag
    /// * `duration` - Wall-clock build duration
    fn record_build_success(&self, recipe: &str, tag: &str, duration: Duration);

    /// Record that a build failed.
    ///
    /// # Arguments
    ///
    /// * `recipe` - Recipe name
    /// * `tag` - Git tag
    fn record_build_failure(&self, recipe: &str, tag: &str);

    /// Start the metrics HTTP server.
    ///
    /// # Arguments
    ///
    /// * `addr` - Socket address to bind to
    ///
    /// # Errors
    ///
    /// Returns [`MetricsError::Server`] if the server fails to start.
    fn serve(
        &self,
        addr: SocketAddr,
    ) -> impl std::future::Future<Output = Result<(), MetricsError>> + Send;
}
