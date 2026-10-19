//! Logging / tracing init (law 10). `RUST_LOG` overrides the default filter.
//! With the `tracy` feature, spans also stream to the Tracy profiler.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// Installs the global subscriber. Call once, first thing in `main`. Safe to call again
/// (later calls are ignored), which keeps tests that share a process happy.
pub fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true));

    #[cfg(feature = "tracy")]
    let registry = registry.with(tracing_tracy::TracyLayer::default());

    if registry.try_init().is_ok() {
        tracing::info!(tracy = cfg!(feature = "tracy"), "logging initialized");
    }
}
