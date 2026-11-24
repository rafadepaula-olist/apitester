use thiserror::Error;

/// Errors produced while applying or negotiating HTTP authentication.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid request URL: {0}")]
    InvalidUrl(String),

    #[error("unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),

    #[error("cryptographic error: {0}")]
    Crypto(String),

    #[error("invalid challenge header: {0}")]
    InvalidChallenge(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("OAuth2 callback error: {0}")]
    OAuth2Callback(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
