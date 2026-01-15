//! # Spotify Confidence OpenFeature Provider
//!
//! A native Rust OpenFeature provider for Confidence that uses the
//! `confidence_resolver` library directly without WASM.
//!
//! ## Features
//!
//! - **Native Performance**: Direct Rust library calls without WASM overhead
//! - **Full Multithreading**: Native async/await with tokio runtime
//! - **Local Evaluation**: Flags are evaluated locally using cached state
//! - **Automatic State Updates**: Background polling for state updates
//! - **Log Telemetry**: Automatic flag resolution and assignment logging
//!
//! ## Example
//!
//! ```rust,no_run
//! use spotify_confidence_openfeature_provider::{ConfidenceProvider, ProviderOptions};
//! use open_feature::OpenFeature;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create and initialize the provider
//!     let options = ProviderOptions::new("your-client-secret");
//!     let mut provider = ConfidenceProvider::new(options)?;
//!     provider.initialize().await?;
//!
//!     // Set as the global provider
//!     OpenFeature::singleton_mut().set_provider(provider).await;
//!
//!     // Use the client
//!     let client = OpenFeature::singleton().create_client();
//!     let value = client.get_bool_value("my-flag.enabled", None, None).await;
//!
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod host;
pub mod logger;
pub mod provider;
pub mod state;
mod version;

pub use error::Error;
pub use provider::{ConfidenceProvider, ProviderOptions};
pub use version::VERSION;

// Re-export commonly used types from open-feature
pub use open_feature::{EvaluationContext, EvaluationError, EvaluationReason};
