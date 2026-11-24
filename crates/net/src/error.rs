use thiserror::Error;

/// Unified error type for `apitester-net`.
#[derive(Debug, Error)]
pub enum NetError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Invalid HTTP method: {0}")]
    InvalidMethod(String),

    #[error("Invalid header `{name}`: {reason}")]
    InvalidHeader { name: String, reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("gRPC error: {0}")]
    Grpc(String),

    #[error("MQTT error: {0}")]
    Mqtt(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Socket.IO error: {0}")]
    SocketIo(String),

    #[error("Channel closed")]
    ChannelClosed,
}

pub type NetResult<T> = Result<T, NetError>;
