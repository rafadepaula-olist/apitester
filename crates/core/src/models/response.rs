use std::borrow::Cow;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::request::KeyValue;

/// Network timing breakdown in milliseconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponseTimings {
    pub dns_ms: f64,
    pub tcp_ms: f64,
    pub tls_ms: f64,
    pub ttfb_ms: f64,
    pub total_ms: f64,
}

/// Result of a single post-response test assertion (`pm.test(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Severity level for script console logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLogLevel {
    #[default]
    Log,
    Info,
    Warn,
    Error,
}

/// A console log entry emitted during pre-request or post-response script execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleLogEntry {
    pub level: ConsoleLogLevel,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

/// An HTTP cookie stored in the cookie jar or returned by a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<DateTime<Utc>>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<String>,
}

impl Default for Cookie {
    fn default() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            domain: String::new(),
            path: "/".to_string(),
            expires: None,
            http_only: false,
            secure: false,
            same_site: None,
        }
    }
}

/// Captured response data from executing a request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<KeyValue>,
    pub body: Vec<u8>,
    pub size_bytes: usize,
    pub timings: ResponseTimings,
    pub cookies: Vec<Cookie>,
    pub test_results: Vec<TestResult>,
    pub console_logs: Vec<ConsoleLogEntry>,
}

impl ResponseData {
    /// Returns the response body decoded as lossy UTF-8 text.
    pub fn body_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Returns the first `Content-Type` header value, if present.
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.enabled && h.key.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.as_str())
    }
}
