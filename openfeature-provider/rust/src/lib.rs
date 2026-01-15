pub mod error;
pub mod host;
pub mod logger;
pub mod materialization;
pub mod provider;
pub mod state;
mod version;

pub use error::Error;
pub use materialization::{
    ConfidenceRemoteMaterializationStore, MaterializationStore, ReadOpType, ReadResultType, WriteOp,
};
pub use provider::{ConfidenceProvider, MaterializationStoreConfig, ProviderOptions};
pub use version::VERSION;

// Re-export commonly used types from open-feature
pub use open_feature::{EvaluationContext, EvaluationError, EvaluationReason};
