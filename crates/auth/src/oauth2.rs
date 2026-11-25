use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use apitester_core::models::auth::{ApiKeyLocation, OAuth2Config};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::{AuthApplication, AuthError};

const MAX_TOKEN_RESPONSE_BYTES: usize = 1024 * 1024; // 1 MiB
const TOKEN_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// PKCE verifier and S256 challenge pair for OAuth 2.0 Authorization Code flow.
#[derive(Clone, PartialEq, Eq)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub method: String,
}

impl fmt::Debug for PkceChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PkceChallenge")
            .field("verifier", &"[REDACTED]")
            .field("challenge", &self.challenge)
            .field("method", &self.method)
            .finish()
    }
}

/// Generated OAuth 2.0 authorization URL along with state and PKCE metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    pub url: String,
    pub state: String,
    pub pkce: PkceChallenge,
}

impl fmt::Debug for AuthorizationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationRequest")
            .field("url", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field("pkce", &self.pkce)
            .finish()
    }
}

/// Standard OAuth 2.0 token endpoint response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuth2TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl fmt::Debug for OAuth2TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2TokenResponse")
            .field("access_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("scope", &self.scope)
            .finish()
    }
}

fn validate_oauth2_url(raw: &str, label: &str) -> Result<Url, AuthError> {
    let url = Url::parse(raw).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" => {
            let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1" | "[::1]"));
            if is_loopback {
                Ok(url)
            } else {
                Err(AuthError::InvalidUrl(format!(
                    "{label} must use https:// (http:// is only permitted for localhost/loopback)"
                )))
            }
        }
        other => Err(AuthError::InvalidUrl(format!(
            "{label} uses unsupported scheme '{other}' (expected https://)"
        ))),
    }
}

/// Verifies an OAuth 2.0 `state` parameter in constant time to prevent CSRF.
pub fn verify_oauth2_state(expected: &str, received: Option<&str>) -> Result<(), AuthError> {
    let Some(got) = received else {
        return Err(AuthError::OAuth2Callback(
            "OAuth2 callback missing state parameter".into(),
        ));
    };
    let a = expected.as_bytes();
    let b = got.as_bytes();
    let mut diff = a.len() ^ b.len();
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= (x ^ y) as usize;
    }
    if diff == 0 {
        Ok(())
    } else {
        Err(AuthError::OAuth2Callback(
            "OAuth2 callback state mismatch".into(),
        ))
    }
}

/// Parameters captured from an OAuth 2.0 localhost redirect callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth2CallbackResult {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Applies a configured OAuth 2.0 access token to an outgoing request.
pub fn apply_oauth2(cfg: &OAuth2Config) -> Result<AuthApplication, AuthError> {
    if cfg.access_token.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth2 access_token must not be empty".into(),
        ));
    }
    match cfg.add_token_to {
        ApiKeyLocation::Header => {
            let prefix = if !cfg.header_prefix.trim().is_empty() {
                cfg.header_prefix.trim()
            } else if !cfg.token_type.trim().is_empty() {
                cfg.token_type.trim()
            } else {
                "Bearer"
            };
            Ok(AuthApplication::with_header(
                "Authorization",
                format!("{prefix} {}", cfg.access_token),
            ))
        }
        ApiKeyLocation::Query => Ok(AuthApplication::with_query(
            "access_token",
            &cfg.access_token,
        )),
    }
}

/// Generates a PKCE S256 challenge and builds the OAuth 2.0 authorization URL.
pub fn build_authorization_url_with_pkce(
    cfg: &OAuth2Config,
) -> Result<AuthorizationRequest, AuthError> {
    if cfg.auth_url.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth2 auth_url must not be empty".into(),
        ));
    }
    if cfg.client_id.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth2 client_id must not be empty".into(),
        ));
    }

    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    let state = if cfg.state.is_empty() {
        let mut state_bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut state_bytes);
        URL_SAFE_NO_PAD.encode(state_bytes)
    } else {
        cfg.state.clone()
    };

    let mut url = validate_oauth2_url(&cfg.auth_url, "OAuth2 auth_url")?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", &cfg.client_id);
        if !cfg.redirect_uri.is_empty() {
            pairs.append_pair("redirect_uri", &cfg.redirect_uri);
        }
        if !cfg.scope.is_empty() {
            pairs.append_pair("scope", &cfg.scope);
        }
        pairs.append_pair("state", &state);
        pairs.append_pair("code_challenge", &challenge);
        pairs.append_pair("code_challenge_method", "S256");
    }

    Ok(AuthorizationRequest {
        url: url.to_string(),
        state,
        pkce: PkceChallenge {
            verifier,
            challenge,
            method: "S256".into(),
        },
    })
}

/// Exchanges an authorization code (with optional PKCE verifier) for an access token.
pub async fn exchange_authorization_code(
    cfg: &OAuth2Config,
    code: &str,
    pkce_verifier: Option<&str>,
) -> Result<OAuth2TokenResponse, AuthError> {
    let mut form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("client_id".to_string(), cfg.client_id.clone()),
    ];
    if !cfg.client_secret.is_empty() {
        form.push(("client_secret".into(), cfg.client_secret.clone()));
    }
    if !cfg.redirect_uri.is_empty() {
        form.push(("redirect_uri".into(), cfg.redirect_uri.clone()));
    }
    if let Some(verifier) = pkce_verifier {
        form.push(("code_verifier".into(), verifier.to_string()));
    }
    post_token_form(&cfg.token_url, &form).await
}

/// Exchanges client credentials for an OAuth 2.0 access token.
pub async fn exchange_client_credentials(
    cfg: &OAuth2Config,
) -> Result<OAuth2TokenResponse, AuthError> {
    if cfg.client_id.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth2 client_id must not be empty".into(),
        ));
    }
    let mut form = vec![
        ("grant_type".to_string(), "client_credentials".to_string()),
        ("client_id".to_string(), cfg.client_id.clone()),
    ];
    if !cfg.client_secret.is_empty() {
        form.push(("client_secret".into(), cfg.client_secret.clone()));
    }
    if !cfg.scope.is_empty() {
        form.push(("scope".into(), cfg.scope.clone()));
    }
    post_token_form(&cfg.token_url, &form).await
}

/// Exchanges resource owner username/password credentials for an OAuth 2.0 access token.
pub async fn exchange_password(
    cfg: &OAuth2Config,
    username: &str,
    password: &str,
) -> Result<OAuth2TokenResponse, AuthError> {
    let mut form = vec![
        ("grant_type".to_string(), "password".to_string()),
        ("username".to_string(), username.to_string()),
        ("password".to_string(), password.to_string()),
    ];
    if !cfg.client_id.is_empty() {
        form.push(("client_id".into(), cfg.client_id.clone()));
    }
    if !cfg.client_secret.is_empty() {
        form.push(("client_secret".into(), cfg.client_secret.clone()));
    }
    if !cfg.scope.is_empty() {
        form.push(("scope".into(), cfg.scope.clone()));
    }
    post_token_form(&cfg.token_url, &form).await
}

/// Refreshes an OAuth 2.0 access token using `cfg.refresh_token`.
pub async fn refresh_access_token(cfg: &OAuth2Config) -> Result<OAuth2TokenResponse, AuthError> {
    let refresh_token = cfg
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AuthError::InvalidConfig("OAuth2 refresh_token must not be empty".into()))?;

    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if !cfg.client_id.is_empty() {
        form.push(("client_id".into(), cfg.client_id.clone()));
    }
    if !cfg.client_secret.is_empty() {
        form.push(("client_secret".into(), cfg.client_secret.clone()));
    }
    if !cfg.scope.is_empty() {
        form.push(("scope".into(), cfg.scope.clone()));
    }
    post_token_form(&cfg.token_url, &form).await
}

/// Listens on `127.0.0.1:<port>` for a single OAuth 2.0 redirect callback and returns its parameters.
pub async fn listen_oauth2_callback(
    port: u16,
    timeout: Duration,
) -> Result<OAuth2CallbackResult, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    listen_oauth2_callback_on_listener(listener, timeout).await
}

/// Accepts a single OAuth 2.0 redirect callback on an already-bound `TcpListener`.
pub async fn listen_oauth2_callback_on_listener(
    listener: TcpListener,
    timeout: Duration,
) -> Result<OAuth2CallbackResult, AuthError> {
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| AuthError::OAuth2Callback("timed out waiting for OAuth2 callback".into()))?;

        let (mut stream, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| AuthError::OAuth2Callback("timed out waiting for OAuth2 callback".into()))??;

        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| AuthError::OAuth2Callback("timed out reading OAuth2 callback request".into()))?;

        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(remaining, stream.read(&mut buf))
            .await
            .map_err(|_| {
                AuthError::OAuth2Callback("timed out reading OAuth2 callback request".into())
            })??;

        if n == 0 {
            continue;
        }

        let req_text = String::from_utf8_lossy(&buf[..n]);
        let Some(request_line) = req_text.lines().next() else {
            continue;
        };
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");

        if !method.eq_ignore_ascii_case("GET") || !target.starts_with('/') {
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            let _ = stream.shutdown().await;
            continue;
        }

        // Ignore browser auxiliary requests (e.g. /favicon.ico) while waiting for the real callback.
        if target == "/favicon.ico" {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            let _ = stream.shutdown().await;
            continue;
        }

        let dummy_url = Url::parse(&format!("http://127.0.0.1{target}"))
            .map_err(|e| AuthError::OAuth2Callback(format!("invalid callback URI: {e}")))?;
        let params: HashMap<String, String> = dummy_url.query_pairs().into_owned().collect();

        let body = "<html><body><h3>Authorization complete. You may close this window.</h3></body></html>";
        let http_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(http_resp.as_bytes()).await;
        let _ = stream.shutdown().await;

        return Ok(OAuth2CallbackResult {
            code: params.get("code").cloned(),
            state: params.get("state").cloned(),
            error: params.get("error").cloned(),
            error_description: params.get("error_description").cloned(),
        });
    }
}

async fn post_token_form(
    token_url: &str,
    form: &[(String, String)],
) -> Result<OAuth2TokenResponse, AuthError> {
    if token_url.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth2 token_url must not be empty".into(),
        ));
    }
    let validated_url = validate_oauth2_url(token_url, "OAuth2 token_url")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(TOKEN_CONNECT_TIMEOUT)
        .timeout(TOKEN_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| AuthError::Http(e.to_string()))?;

    let mut resp = client
        .post(validated_url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?;

    let status = resp.status();
    let mut raw = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AuthError::Http(e.to_string()))?
    {
        if raw.len().saturating_add(chunk.len()) > MAX_TOKEN_RESPONSE_BYTES {
            return Err(AuthError::Http(format!(
                "token endpoint response exceeded {MAX_TOKEN_RESPONSE_BYTES} bytes"
            )));
        }
        raw.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&raw).into_owned();

    if !status.is_success() {
        let preview: String = body.chars().take(512).collect();
        return Err(AuthError::Http(format!(
            "token endpoint returned {status}: {preview}"
        )));
    }

    serde_json::from_str(&body)
        .map_err(|e| AuthError::Http(format!("failed to parse token response JSON: {e}")))
}
