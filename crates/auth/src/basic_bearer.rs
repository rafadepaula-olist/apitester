use apitester_core::models::auth::ApiKeyLocation;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::{AuthApplication, AuthError};

pub fn apply_bearer(token: &str) -> Result<AuthApplication, AuthError> {
    if token.is_empty() {
        return Err(AuthError::InvalidConfig(
            "bearer token must not be empty".into(),
        ));
    }
    Ok(AuthApplication::with_header(
        "Authorization",
        format!("Bearer {token}"),
    ))
}

pub fn apply_basic(username: &str, password: &str) -> Result<AuthApplication, AuthError> {
    let encoded = STANDARD.encode(format!("{username}:{password}"));
    Ok(AuthApplication::with_header(
        "Authorization",
        format!("Basic {encoded}"),
    ))
}

pub fn apply_api_key(
    key: &str,
    value: &str,
    add_to: ApiKeyLocation,
) -> Result<AuthApplication, AuthError> {
    if key.is_empty() {
        return Err(AuthError::InvalidConfig(
            "API key name must not be empty".into(),
        ));
    }
    match add_to {
        ApiKeyLocation::Header => Ok(AuthApplication::with_header(key, value)),
        ApiKeyLocation::Query => Ok(AuthApplication::with_query(key, value)),
    }
}
