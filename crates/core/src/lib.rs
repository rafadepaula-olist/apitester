//! Core data models, variable interpolation engine, and hybrid storage for apitester.

pub mod error;
pub mod models;
pub mod storage;
pub mod variables;

pub use error::{CoreError, CoreResult};
pub use models::*;
pub use storage::{HybridVault, JsonStore, KeyringVault, MemoryVault, SqliteStore, VaultStore};
pub use variables::{VariableContext, VariableResolver, VariableScope};
