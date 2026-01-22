//! Error types for the Confidence OpenFeature provider.

use thiserror::Error;

/// Errors that can occur in the Confidence OpenFeature provider.
#[derive(Debug, Error)]
pub enum Error {
    /// Failed to fetch state from CDN.
    #[error("failed to fetch state: {0}")]
    StateFetch(String),

    /// Failed to parse state protobuf.
    #[error("failed to parse state: {0}")]
    StateParse(String),

    /// Provider is not initialized.
    #[error("provider not initialized")]
    NotInitialized,

    /// Failed to resolve flag.
    #[error("failed to resolve flag: {0}")]
    Resolve(String),

    /// Flag not found.
    #[error("flag not found: {0}")]
    FlagNotFound(String),

    /// Type mismatch when extracting flag value.
    #[error("type mismatch: {0}")]
    TypeMismatch(String),

    /// Path not found in flag value.
    #[error("path not found: {0}")]
    PathNotFound(String),

    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// Protobuf encoding/decoding error.
    #[error("protobuf error: {0}")]
    Proto(String),

    /// Materialization store error.
    #[error("materialization error: {0}")]
    Materialization(String),
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e.to_string())
    }
}

impl From<reqwest_middleware::Error> for Error {
    fn from(e: reqwest_middleware::Error) -> Self {
        Error::Http(e.to_string())
    }
}

/// Result type alias for the provider.
pub type Result<T> = std::result::Result<T, Error>;
