use thiserror::Error;

/// Errors returned by collection runner data parsers and execution helpers.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("failed to parse CSV data: {0}")]
    InvalidCsv(String),

    #[error("failed to parse JSON data: {0}")]
    InvalidJson(#[from] serde_json::Error),
}
