pub mod error;
mod gateway;
pub mod host;
pub mod logger;
pub mod materialization;
pub mod provider;
pub mod state;
mod version;

#[cfg(test)]
pub mod test_utils;

pub use error::{Error, Result};
pub use materialization::{MaterializationStore, ReadOpType, ReadResultType, WriteOp};
pub use provider::{ConfidenceProvider, MaterializationStoreConfig, ProviderOptions};
pub use version::VERSION;

// Re-export commonly used types from open-feature
pub use open_feature::{EvaluationContext, EvaluationError, EvaluationReason};
