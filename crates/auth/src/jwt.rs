use apitester_core::models::auth::{ApiKeyLocation, JwtConfig};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::{Sha256, Sha384, Sha512};

use crate::{AuthApplication, AuthError};

type HmacSha256 = Hmac<Sha256>;
type HmacSha384 = Hmac<Sha384>;
type HmacSha512 = Hmac<Sha512>;

pub fn apply_jwt(cfg: &JwtConfig) -> Result<AuthApplication, AuthError> {
    let alg = if cfg.algorithm.is_empty() {
        "HS256"
    } else {
        cfg.algorithm.as_str()
    };

    let secret_bytes = if cfg.is_secret_base64 {
        STANDARD
            .decode(cfg.secret.trim())
            .map_err(|e| AuthError::InvalidConfig(format!("invalid base64 JWT secret: {e}")))?
    } else {
        cfg.secret.as_bytes().to_vec()
    };
    if secret_bytes.is_empty() {
        return Err(AuthError::InvalidConfig(
            "JWT secret must not be empty".into(),
        ));
    }

    let mut header_obj = Map::new();
    header_obj.insert("alg".into(), Value::String(alg.to_string()));
    header_obj.insert("typ".into(), Value::String("JWT".into()));

    if let Some(ref custom) = cfg.custom_headers {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            let parsed: Value = serde_json::from_str(trimmed).map_err(|e| {
                AuthError::InvalidConfig(format!("invalid JWT custom_headers JSON: {e}"))
            })?;
            let obj = parsed.as_object().ok_or_else(|| {
                AuthError::InvalidConfig("JWT custom_headers must be a JSON object".into())
            })?;
            for (k, v) in obj {
                if !k.eq_ignore_ascii_case("alg") {
                    header_obj.insert(k.clone(), v.clone());
                }
            }
        }
    }

    let payload_str = if cfg.payload.trim().is_empty() {
        "{}"
    } else {
        cfg.payload.trim()
    };
    let payload_val: Value = serde_json::from_str(payload_str)
        .map_err(|e| AuthError::InvalidConfig(format!("invalid JWT payload JSON: {e}")))?;

    let header_json =
        serde_json::to_vec(&Value::Object(header_obj)).map_err(|e| AuthError::Crypto(e.to_string()))?;
    let payload_json =
        serde_json::to_vec(&payload_val).map_err(|e| AuthError::Crypto(e.to_string()))?;

    let encoded_header = URL_SAFE_NO_PAD.encode(header_json);
    let encoded_payload = URL_SAFE_NO_PAD.encode(payload_json);
    let signing_input = format!("{encoded_header}.{encoded_payload}");

    let signature = sign_hmac(alg, &secret_bytes, signing_input.as_bytes())?;
    let encoded_sig = URL_SAFE_NO_PAD.encode(signature);
    let token = format!("{signing_input}.{encoded_sig}");

    match cfg.add_to {
        ApiKeyLocation::Header => {
            let prefix = cfg.header_prefix.trim();
            let header_val = if prefix.is_empty() {
                token
            } else {
                format!("{prefix} {token}")
            };
            Ok(AuthApplication::with_header("Authorization", header_val))
        }
        ApiKeyLocation::Query => {
            let key = if cfg.query_param_key.is_empty() {
                "token"
            } else {
                cfg.query_param_key.as_str()
            };
            Ok(AuthApplication::with_query(key, token))
        }
    }
}

fn sign_hmac(alg: &str, secret: &[u8], data: &[u8]) -> Result<Vec<u8>, AuthError> {
    match alg.to_ascii_uppercase().as_str() {
        "HS256" => {
            let mut mac =
                HmacSha256::new_from_slice(secret).map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        "HS384" => {
            let mut mac =
                HmacSha384::new_from_slice(secret).map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        "HS512" => {
            let mut mac =
                HmacSha512::new_from_slice(secret).map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        other => Err(AuthError::UnsupportedAlgorithm(format!(
            "JWT algorithm {other} is not supported (expected HS256, HS384, or HS512)"
        ))),
    }
}
