use apitester_core::models::auth::AsapConfig;
use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{AuthApplication, AuthError};

pub fn apply_asap(cfg: &AsapConfig) -> Result<AuthApplication, AuthError> {
    if cfg.issuer.is_empty() || cfg.kid.is_empty() || cfg.audience.is_empty() {
        return Err(AuthError::InvalidConfig(
            "ASAP issuer, kid, and audience must not be empty".into(),
        ));
    }
    if cfg.private_key.is_empty() {
        return Err(AuthError::InvalidConfig(
            "ASAP private_key must not be empty".into(),
        ));
    }

    let alg_str = if cfg.alg.is_empty() {
        "RS256"
    } else {
        cfg.alg.as_str()
    };
    let algorithm = parse_algorithm(alg_str)?;

    let mut header = Header::new(algorithm);
    header.typ = Some("JWT".into());
    header.kid = Some(cfg.kid.clone());

    let now = Utc::now().timestamp();
    let expiry = if cfg.expiry == 0 { 60 } else { cfg.expiry } as i64;

    let mut claims = Map::new();
    if !cfg.claims.trim().is_empty() {
        let extra: Value = serde_json::from_str(cfg.claims.trim())
            .map_err(|e| AuthError::InvalidConfig(format!("invalid ASAP claims JSON: {e}")))?;
        let obj = extra.as_object().ok_or_else(|| {
            AuthError::InvalidConfig("ASAP claims must be a JSON object".into())
        })?;
        for (k, v) in obj {
            claims.insert(k.clone(), v.clone());
        }
    }

    claims.insert("iss".into(), Value::String(cfg.issuer.clone()));
    claims.insert("aud".into(), Value::String(cfg.audience.clone()));
    claims.insert("iat".into(), Value::Number(now.into()));
    claims.insert("exp".into(), Value::Number((now + expiry).into()));
    claims.insert("jti".into(), Value::String(Uuid::new_v4().to_string()));
    if !cfg.subject.is_empty() {
        claims.insert("sub".into(), Value::String(cfg.subject.clone()));
    }

    let key = encoding_key(algorithm, &cfg.private_key)?;
    let token = jsonwebtoken::encode(&header, &Value::Object(claims), &key)
        .map_err(|e| AuthError::Crypto(format!("ASAP JWT signing failed: {e}")))?;

    Ok(AuthApplication::with_header(
        "Authorization",
        format!("Bearer {token}"),
    ))
}

fn parse_algorithm(alg: &str) -> Result<Algorithm, AuthError> {
    match alg.to_ascii_uppercase().as_str() {
        "RS256" => Ok(Algorithm::RS256),
        "RS384" => Ok(Algorithm::RS384),
        "RS512" => Ok(Algorithm::RS512),
        "PS256" => Ok(Algorithm::PS256),
        "PS384" => Ok(Algorithm::PS384),
        "PS512" => Ok(Algorithm::PS512),
        "ES256" => Ok(Algorithm::ES256),
        "ES384" => Ok(Algorithm::ES384),
        "HS256" => Ok(Algorithm::HS256),
        "HS384" => Ok(Algorithm::HS384),
        "HS512" => Ok(Algorithm::HS512),
        other => Err(AuthError::UnsupportedAlgorithm(format!(
            "ASAP algorithm {other} is not supported"
        ))),
    }
}

fn encoding_key(alg: Algorithm, key_material: &str) -> Result<EncodingKey, AuthError> {
    match alg {
        Algorithm::RS256
        | Algorithm::RS384
        | Algorithm::RS512
        | Algorithm::PS256
        | Algorithm::PS384
        | Algorithm::PS512 => EncodingKey::from_rsa_pem(key_material.as_bytes())
            .map_err(|e| AuthError::Crypto(format!("invalid RSA PEM key: {e}"))),
        Algorithm::ES256 | Algorithm::ES384 => EncodingKey::from_ec_pem(key_material.as_bytes())
            .map_err(|e| AuthError::Crypto(format!("invalid EC PEM key: {e}"))),
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
            Ok(EncodingKey::from_secret(key_material.as_bytes()))
        }
        Algorithm::EdDSA => EncodingKey::from_ed_pem(key_material.as_bytes())
            .map_err(|e| AuthError::Crypto(format!("invalid EdDSA PEM key: {e}"))),
    }
}
