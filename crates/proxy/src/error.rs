use thiserror::Error;

/// Errors returned by the local capture proxy and CA utilities.
#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("certificate error: {0}")]
    Certificate(#[from] rcgen::Error),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),
}
