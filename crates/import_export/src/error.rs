use thiserror::Error;

/// Errors returned by import/export operations.
#[derive(Debug, Error)]
pub enum ImportExportError {
    #[error("JSON parse/serialize error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("invalid format: {0}")]
    InvalidFormat(String),

    #[error("cURL parse error: {0}")]
    CurlParse(String),

    #[error("unsupported version: {0}")]
    UnsupportedVersion(String),
}

pub type ImportExportResult<T> = Result<T, ImportExportError>;
