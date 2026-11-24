use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::request::{HttpMethod, RequestItem};
use super::response::ResponseData;

/// A single scoped variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Variable {
    pub key: String,
    pub value: String,
    pub enabled: bool,
    pub secret: bool,
}

impl Default for Variable {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            enabled: true,
            secret: false,
        }
    }
}

impl Variable {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            enabled: true,
            secret: false,
        }
    }

    pub fn secret(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            enabled: true,
            secret: true,
        }
    }
}

/// Named set of environment variables (e.g., Development, Staging, Production).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub variables: Vec<Variable>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for Environment {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: "New Environment".to_string(),
            variables: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// A recorded request execution in local history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub method: HttpMethod,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<f64>,
    pub request: RequestItem,
    pub response: Option<ResponseData>,
}

/// Per-host client TLS certificate configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientCertConfig {
    pub host: String,
    pub cert_path: String,
    pub key_path: String,
    pub passphrase: Option<String>,
    pub ca_path: Option<String>,
    pub enabled: bool,
}

impl Default for ClientCertConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            cert_path: String::new(),
            key_path: String::new(),
            passphrase: None,
            ca_path: None,
            enabled: true,
        }
    }
}

/// Outbound proxy protocol type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyType {
    #[default]
    Http,
    Https,
    Socks5,
}

/// Outbound proxy configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub enabled: bool,
    pub proxy_type: ProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub bypass: Vec<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_type: ProxyType::Http,
            host: "127.0.0.1".to_string(),
            port: 8080,
            username: None,
            password: None,
            bypass: Vec::new(),
        }
    }
}

/// UI color theme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

/// Global application settings stored in SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub verify_ssl: bool,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub timeout_ms: u64,
    pub proxy: ProxyConfig,
    pub client_certs: Vec<ClientCertConfig>,
    pub global_variables: Vec<Variable>,
    pub active_environment_id: Option<Uuid>,
    pub history_limit: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            verify_ssl: true,
            follow_redirects: true,
            max_redirects: 10,
            timeout_ms: 30_000,
            proxy: ProxyConfig::default(),
            client_certs: Vec::new(),
            global_variables: Vec::new(),
            active_environment_id: None,
            history_limit: 500,
        }
    }
}
