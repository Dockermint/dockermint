//! Prometheus metrics backend exposed via an [`axum`] HTTP server.
//!
//! Metrics are stored in-memory and rendered in the
//! [Prometheus text exposition format](https://prometheus.io/docs/instrumenting/exposition_formats/#text-based-format)
//! on each `/metrics` scrape.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::error::MetricsError;
use crate::metrics::MetricsCollector;

// ===========================================================================
// Internal types
// ===========================================================================

/// Aggregated counters for a single recipe.
#[derive(Debug, Default, Clone)]
struct RecipeMetrics {
    builds_started: u64,
    builds_succeeded: u64,
    builds_failed: u64,
    build_duration_seconds_total: f64,
}

/// Thread-safe handle to the metrics store.
type MetricsStore = Arc<RwLock<HashMap<String, RecipeMetrics>>>;

// ===========================================================================
// PrometheusCollector
// ===========================================================================

/// Prometheus-compatible metrics collector.
///
/// Tracks per-recipe build counters and cumulative build duration, then
/// exposes them via a lightweight [`axum`] HTTP endpoint.
#[derive(Debug, Clone)]
pub struct PrometheusCollector {
    store: MetricsStore,
}

impl PrometheusCollector {
    /// Create a new Prometheus collector with an empty metrics store.
    ///
    /// # Errors
    ///
    /// Returns [`MetricsError::Registration`] if metric registration
    /// fails.
    pub fn new() -> Result<Self, MetricsError> {
        Ok(Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

impl MetricsCollector for PrometheusCollector {
    fn record_build_start(&self, recipe: &str, _tag: &str) {
        match self.store.write() {
            Ok(mut map) => {
                map.entry(recipe.to_owned()).or_default().builds_started += 1;
            },
            Err(e) => {
                tracing::error!(
                    recipe,
                    "metrics store lock poisoned on record_build_start: {e}"
                );
            },
        }
    }

    fn record_build_success(&self, recipe: &str, _tag: &str, duration: Duration) {
        match self.store.write() {
            Ok(mut map) => {
                let entry = map.entry(recipe.to_owned()).or_default();
                entry.builds_succeeded += 1;
                entry.build_duration_seconds_total += duration.as_secs_f64();
            },
            Err(e) => {
                tracing::error!(
                    recipe,
                    "metrics store lock poisoned on record_build_success: {e}"
                );
            },
        }
    }

    fn record_build_failure(&self, recipe: &str, _tag: &str) {
        match self.store.write() {
            Ok(mut map) => {
                map.entry(recipe.to_owned()).or_default().builds_failed += 1;
            },
            Err(e) => {
                tracing::error!(
                    recipe,
                    "metrics store lock poisoned on record_build_failure: {e}"
                );
            },
        }
    }

    async fn serve(&self, addr: SocketAddr) -> Result<(), MetricsError> {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(Arc::clone(&self.store));

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| MetricsError::Server(e.to_string()))?;

        tracing::info!(%addr, "prometheus metrics server listening");

        axum::serve(listener, app)
            .await
            .map_err(|e| MetricsError::Server(e.to_string()))
    }
}

// ===========================================================================
// HTTP handler
// ===========================================================================

/// Prometheus text exposition format content type.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Render all collected metrics in Prometheus text format.
async fn metrics_handler(State(store): State<MetricsStore>) -> impl IntoResponse {
    let map = store.read().unwrap_or_else(|e| e.into_inner());

    let mut buf = String::with_capacity(512);

    // -- builds started ------------------------------------------------
    buf.push_str(
        "# HELP dockermint_builds_started_total Total number of builds started.\n\
         # TYPE dockermint_builds_started_total counter\n",
    );
    for (recipe, m) in &*map {
        let _ = writeln!(
            buf,
            "dockermint_builds_started_total{{recipe=\"{recipe}\"}} {}",
            m.builds_started,
        );
    }

    // -- builds succeeded ----------------------------------------------
    buf.push_str(
        "# HELP dockermint_builds_succeeded_total Total number of successful builds.\n\
         # TYPE dockermint_builds_succeeded_total counter\n",
    );
    for (recipe, m) in &*map {
        let _ = writeln!(
            buf,
            "dockermint_builds_succeeded_total{{recipe=\"{recipe}\"}} {}",
            m.builds_succeeded,
        );
    }

    // -- builds failed -------------------------------------------------
    buf.push_str(
        "# HELP dockermint_builds_failed_total Total number of failed builds.\n\
         # TYPE dockermint_builds_failed_total counter\n",
    );
    for (recipe, m) in &*map {
        let _ = writeln!(
            buf,
            "dockermint_builds_failed_total{{recipe=\"{recipe}\"}} {}",
            m.builds_failed,
        );
    }

    // -- build duration ------------------------------------------------
    buf.push_str(
        "# HELP dockermint_build_duration_seconds_total Cumulative build duration in seconds.\n\
         # TYPE dockermint_build_duration_seconds_total counter\n",
    );
    for (recipe, m) in &*map {
        let _ = writeln!(
            buf,
            "dockermint_build_duration_seconds_total{{recipe=\"{recipe}\"}} {:.6}",
            m.build_duration_seconds_total,
        );
    }

    ([(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], buf)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_collector_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrometheusCollector>();
    }

    #[test]
    fn new_returns_ok() {
        let collector = PrometheusCollector::new();
        assert!(collector.is_ok());
    }

    #[test]
    fn record_build_start_increments() {
        let c = PrometheusCollector::new().expect("collector");
        c.record_build_start("cosmos", "v1.0.0");
        c.record_build_start("cosmos", "v1.1.0");
        c.record_build_start("osmosis", "v2.0.0");

        let map = c.store.read().expect("lock");
        assert_eq!(map["cosmos"].builds_started, 2);
        assert_eq!(map["osmosis"].builds_started, 1);
    }

    #[test]
    fn record_build_success_increments_and_accumulates_duration() {
        let c = PrometheusCollector::new().expect("collector");
        c.record_build_success("cosmos", "v1.0.0", Duration::from_secs(10));
        c.record_build_success("cosmos", "v1.1.0", Duration::from_millis(500));

        let map = c.store.read().expect("lock");
        assert_eq!(map["cosmos"].builds_succeeded, 2);
        let expected = 10.0 + 0.5;
        assert!((map["cosmos"].build_duration_seconds_total - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn record_build_failure_increments() {
        let c = PrometheusCollector::new().expect("collector");
        c.record_build_failure("cosmos", "v1.0.0");

        let map = c.store.read().expect("lock");
        assert_eq!(map["cosmos"].builds_failed, 1);
    }

    #[tokio::test]
    async fn metrics_handler_renders_prometheus_format() {
        let store: MetricsStore = Arc::new(RwLock::new(HashMap::new()));
        {
            let mut map = store.write().expect("lock");
            map.insert(
                "cosmos".to_owned(),
                RecipeMetrics {
                    builds_started: 5,
                    builds_succeeded: 3,
                    builds_failed: 2,
                    build_duration_seconds_total: 42.5,
                },
            );
        }

        let response = metrics_handler(State(store)).await.into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let text = String::from_utf8(body.to_vec()).expect("utf8");

        assert!(text.contains("dockermint_builds_started_total{recipe=\"cosmos\"} 5"));
        assert!(text.contains("dockermint_builds_succeeded_total{recipe=\"cosmos\"} 3"));
        assert!(text.contains("dockermint_builds_failed_total{recipe=\"cosmos\"} 2"));
        assert!(
            text.contains("dockermint_build_duration_seconds_total{recipe=\"cosmos\"} 42.500000")
        );
    }
}
