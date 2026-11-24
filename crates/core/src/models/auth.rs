use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where an auth credential (API key, token, signature) is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

/// Supported OAuth 2.0 grant types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuth2GrantType {
    #[default]
    AuthorizationCode,
    AuthorizationCodePkce,
    ClientCredentials,
    Password,
    Implicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth2Config {
    pub grant_type: OAuth2GrantType,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub header_prefix: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub state: String,
    pub add_token_to: ApiKeyLocation,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AwsV4Config {
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub service: String,
    pub session_token: Option<String>,
    pub add_to: ApiKeyLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JwtConfig {
    pub algorithm: String,
    pub secret: String,
    pub is_secret_base64: bool,
    pub payload: String,
    pub header_prefix: String,
    pub query_param_key: String,
    pub add_to: ApiKeyLocation,
    pub custom_headers: Option<String>,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            algorithm: "HS256".to_string(),
            secret: String::new(),
            is_secret_base64: false,
            payload: "{}".to_string(),
            header_prefix: "Bearer".to_string(),
            query_param_key: "token".to_string(),
            add_to: ApiKeyLocation::Header,
            custom_headers: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DigestConfig {
    pub username: String,
    pub password: String,
    pub realm: String,
    pub nonce: String,
    pub algorithm: String,
    pub qop: String,
    pub nonce_count: String,
    pub client_nonce: String,
    pub opaque: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth1Config {
    pub consumer_key: String,
    pub consumer_secret: String,
    pub token: String,
    pub token_secret: String,
    pub signature_method: String,
    pub timestamp: Option<String>,
    pub nonce: Option<String>,
    pub version: String,
    pub realm: Option<String>,
    pub add_params_to_header: bool,
    pub add_empty_params_to_sign: bool,
}

impl Default for OAuth1Config {
    fn default() -> Self {
        Self {
            consumer_key: String::new(),
            consumer_secret: String::new(),
            token: String::new(),
            token_secret: String::new(),
            signature_method: "HMAC-SHA1".to_string(),
            timestamp: None,
            nonce: None,
            version: "1.0".to_string(),
            realm: None,
            add_params_to_header: true,
            add_empty_params_to_sign: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NtlmConfig {
    pub username: String,
    pub password: String,
    pub domain: String,
    pub workstation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HawkConfig {
    pub auth_id: String,
    pub auth_key: String,
    pub algorithm: String,
    pub user: String,
    pub nonce: String,
    pub extra_data: String,
    pub app: String,
    pub delegation: String,
    pub timestamp: Option<String>,
    pub include_payload_hash: bool,
}

impl Default for HawkConfig {
    fn default() -> Self {
        Self {
            auth_id: String::new(),
            auth_key: String::new(),
            algorithm: "sha256".to_string(),
            user: String::new(),
            nonce: String::new(),
            extra_data: String::new(),
            app: String::new(),
            delegation: String::new(),
            timestamp: None,
            include_payload_hash: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeGridConfig {
    pub access_token: String,
    pub client_token: String,
    pub client_secret: String,
    pub nonce: String,
    pub timestamp: Option<String>,
    pub base_url: String,
    pub headers_to_sign: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AsapConfig {
    pub alg: String,
    pub issuer: String,
    pub kid: String,
    pub audience: String,
    pub subject: String,
    pub private_key: String,
    pub claims: String,
    pub expiry: u64,
}

impl Default for AsapConfig {
    fn default() -> Self {
        Self {
            alg: "RS256".to_string(),
            issuer: String::new(),
            kid: String::new(),
            audience: String::new(),
            subject: String::new(),
            private_key: String::new(),
            claims: String::new(),
            expiry: 60,
        }
    }
}

/// Authentication configuration covering all 13 Postman auth types plus `Inherit`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    Inherit,
    NoAuth,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        #[serde(default)]
        add_to: ApiKeyLocation,
    },
    OAuth2(OAuth2Config),
    AwsV4(AwsV4Config),
    Jwt(JwtConfig),
    Digest(DigestConfig),
    OAuth1(OAuth1Config),
    Ntlm(NtlmConfig),
    Hawk(HawkConfig),
    EdgeGrid(EdgeGridConfig),
    Asap(AsapConfig),
}

static NO_AUTH: AuthConfig = AuthConfig::NoAuth;

/// Resolves effective authentication across an inheritance chain ordered from
/// most-specific to least-specific (e.g., `[&request.auth, &folder.auth, &collection.auth]`).
///
/// Returns the first non-`Inherit` variant, or `&AuthConfig::NoAuth` if every entry is `Inherit`.
pub fn resolve_auth<'a>(chain: &[&'a AuthConfig]) -> &'a AuthConfig {
    for auth in chain {
        if !matches!(auth, AuthConfig::Inherit) {
            return auth;
        }
    }
    &NO_AUTH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_auth_picks_first_non_inherit() {
        let req = AuthConfig::Inherit;
        let folder = AuthConfig::Bearer {
            token: "folder-token".into(),
        };
        let collection = AuthConfig::Basic {
            username: "u".into(),
            password: "p".into(),
        };

        assert_eq!(resolve_auth(&[&req, &folder, &collection]), &folder);
    }

    #[test]
    fn resolve_auth_respects_explicit_no_auth_override() {
        let req = AuthConfig::NoAuth;
        let collection = AuthConfig::Bearer {
            token: "col-token".into(),
        };

        assert_eq!(resolve_auth(&[&req, &collection]), &AuthConfig::NoAuth);
    }

    #[test]
    fn resolve_auth_defaults_to_no_auth_when_all_inherit() {
        assert_eq!(
            resolve_auth(&[&AuthConfig::Inherit, &AuthConfig::Inherit]),
            &AuthConfig::NoAuth
        );
        assert_eq!(resolve_auth(&[]), &AuthConfig::NoAuth);
    }

    #[test]
    fn all_auth_variants_round_trip_json() {
        let variants = vec![
            AuthConfig::Inherit,
            AuthConfig::NoAuth,
            AuthConfig::Bearer {
                token: "tok".into(),
            },
            AuthConfig::Basic {
                username: "alice".into(),
                password: "secret".into(),
            },
            AuthConfig::ApiKey {
                key: "X-Api-Key".into(),
                value: "123".into(),
                add_to: ApiKeyLocation::Header,
            },
            AuthConfig::OAuth2(OAuth2Config::default()),
            AuthConfig::AwsV4(AwsV4Config::default()),
            AuthConfig::Jwt(JwtConfig::default()),
            AuthConfig::Digest(DigestConfig::default()),
            AuthConfig::OAuth1(OAuth1Config::default()),
            AuthConfig::Ntlm(NtlmConfig::default()),
            AuthConfig::Hawk(HawkConfig::default()),
            AuthConfig::EdgeGrid(EdgeGridConfig::default()),
            AuthConfig::Asap(AsapConfig::default()),
        ];

        for variant in variants {
            let json = serde_json::to_string(&variant).expect("serialize auth");
            let back: AuthConfig = serde_json::from_str(&json).expect("deserialize auth");
            assert_eq!(back, variant);
        }
    }
}
