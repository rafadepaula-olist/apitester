//! Authentication providers for apitester covering all 13 Postman auth types plus `Inherit`.

pub mod asap;
pub mod aws_v4;
pub mod basic_bearer;
pub mod digest;
pub mod edgegrid;
pub mod error;
pub mod hawk;
pub mod jwt;
pub mod ntlm;
pub mod oauth1;
pub mod oauth2;

pub use apitester_core::models::auth::{
    ApiKeyLocation, AsapConfig, AuthConfig, AwsV4Config, DigestConfig, EdgeGridConfig, HawkConfig,
    JwtConfig, NtlmConfig, OAuth1Config, OAuth2Config, OAuth2GrantType, resolve_auth,
};
pub use digest::apply_digest_challenge;
pub use error::AuthError;
pub use ntlm::{apply_ntlm_challenge, build_type1_negotiate, build_type3_authenticate};
pub use oauth2::{
    AuthorizationRequest, OAuth2CallbackResult, OAuth2TokenResponse, PkceChallenge,
    build_authorization_url_with_pkce, exchange_authorization_code, exchange_client_credentials,
    exchange_password, listen_oauth2_callback, listen_oauth2_callback_on_listener,
    refresh_access_token, verify_oauth2_state,
};

/// Alias for [`DigestConfig`] matching external caller naming conventions.
pub type DigestAuthConfig = DigestConfig;

/// Request details needed to sign or authenticate an outgoing HTTP request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthRequestContext {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Headers and query parameters produced by applying an authentication scheme.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthApplication {
    pub headers: Vec<(String, String)>,
    pub query_params: Vec<(String, String)>,
}

impl std::fmt::Debug for AuthApplication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, _)| (k.as_str(), "[REDACTED]"))
            .collect();
        let query_params: Vec<(&str, &str)> = self
            .query_params
            .iter()
            .map(|(k, _)| (k.as_str(), "[REDACTED]"))
            .collect();
        f.debug_struct("AuthApplication")
            .field("headers", &headers)
            .field("query_params", &query_params)
            .finish()
    }
}

impl AuthApplication {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_header(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            headers: vec![(key.into(), value.into())],
            query_params: Vec::new(),
        }
    }

    pub fn with_query(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            headers: Vec::new(),
            query_params: vec![(key.into(), value.into())],
        }
    }
}

/// Applies the configured authentication scheme to the given request context.
pub fn apply_auth(
    auth: &AuthConfig,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    match auth {
        AuthConfig::NoAuth | AuthConfig::Inherit => Ok(AuthApplication::empty()),
        AuthConfig::Bearer { token } => basic_bearer::apply_bearer(token),
        AuthConfig::Basic { username, password } => basic_bearer::apply_basic(username, password),
        AuthConfig::ApiKey { key, value, add_to } => {
            basic_bearer::apply_api_key(key, value, *add_to)
        }
        AuthConfig::OAuth2(cfg) => oauth2::apply_oauth2(cfg),
        AuthConfig::AwsV4(cfg) => aws_v4::apply_aws_v4(cfg, ctx),
        AuthConfig::Jwt(cfg) => jwt::apply_jwt(cfg),
        AuthConfig::Digest(cfg) => digest::apply_digest(cfg, ctx),
        AuthConfig::OAuth1(cfg) => oauth1::apply_oauth1(cfg, ctx),
        AuthConfig::Ntlm(cfg) => ntlm::apply_ntlm(cfg),
        AuthConfig::Hawk(cfg) => hawk::apply_hawk(cfg, ctx),
        AuthConfig::EdgeGrid(cfg) => edgegrid::apply_edgegrid(cfg, ctx),
        AuthConfig::Asap(cfg) => asap::apply_asap(cfg),
    }
}
