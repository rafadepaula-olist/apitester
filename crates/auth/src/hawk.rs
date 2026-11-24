use apitester_core::models::auth::HawkConfig;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{AuthApplication, AuthError, AuthRequestContext};

type HmacSha1 = Hmac<Sha1>;
type HmacSha256 = Hmac<Sha256>;

pub fn apply_hawk(
    cfg: &HawkConfig,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    if cfg.auth_id.is_empty() || cfg.auth_key.is_empty() {
        return Err(AuthError::InvalidConfig(
            "Hawk auth_id and auth_key must not be empty".into(),
        ));
    }

    let parsed = Url::parse(&ctx.url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AuthError::InvalidUrl("URL is missing host".into()))?
        .to_ascii_lowercase();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| AuthError::InvalidUrl("URL has unknown default port".into()))?;
    let resource = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_string(),
    };
    let method = ctx.method.to_ascii_uppercase();

    let alg = if cfg.algorithm.is_empty() {
        "sha256"
    } else {
        cfg.algorithm.as_str()
    };
    let alg_lower = alg.to_ascii_lowercase();
    if alg_lower != "sha256" && alg_lower != "sha1" {
        return Err(AuthError::UnsupportedAlgorithm(format!(
            "Hawk algorithm {alg} is not supported (expected sha256 or sha1)"
        )));
    }

    let ts = cfg
        .timestamp
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Utc::now().timestamp().to_string());
    let nonce = if cfg.nonce.is_empty() {
        random_nonce_6()
    } else {
        cfg.nonce.clone()
    };

    let payload_hash = if cfg.include_payload_hash {
        let content_type = ctx
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();

        let mut payload_buf = Vec::new();
        payload_buf.extend_from_slice(b"hawk.1.payload\n");
        payload_buf.extend_from_slice(content_type.as_bytes());
        payload_buf.push(b'\n');
        payload_buf.extend_from_slice(&ctx.body);
        payload_buf.push(b'\n');

        let digest = hash_bytes(&alg_lower, &payload_buf)?;
        Some(STANDARD.encode(digest))
    } else {
        None
    };

    let mut normalized = format!(
        "hawk.1.header\n{ts}\n{nonce}\n{method}\n{resource}\n{host}\n{port}\n{}\n{}\n",
        payload_hash.as_deref().unwrap_or(""),
        cfg.extra_data
    );
    if !cfg.app.is_empty() {
        normalized.push_str(&cfg.app);
        normalized.push('\n');
        normalized.push_str(&cfg.delegation);
        normalized.push('\n');
    }

    let mac_bytes = hmac_bytes(&alg_lower, cfg.auth_key.as_bytes(), normalized.as_bytes())?;
    let mac = STANDARD.encode(mac_bytes);

    let mut parts = vec![
        format!("id=\"{}\"", escape_quoted(&cfg.auth_id)),
        format!("ts=\"{ts}\""),
        format!("nonce=\"{}\"", escape_quoted(&nonce)),
    ];
    if let Some(ref h) = payload_hash {
        parts.push(format!("hash=\"{h}\""));
    }
    if !cfg.extra_data.is_empty() {
        parts.push(format!("ext=\"{}\"", escape_quoted(&cfg.extra_data)));
    }
    parts.push(format!("mac=\"{mac}\""));
    if !cfg.app.is_empty() {
        parts.push(format!("app=\"{}\"", escape_quoted(&cfg.app)));
        if !cfg.delegation.is_empty() {
            parts.push(format!("dlg=\"{}\"", escape_quoted(&cfg.delegation)));
        }
    }

    Ok(AuthApplication::with_header(
        "Authorization",
        format!("Hawk {}", parts.join(", ")),
    ))
}

fn hash_bytes(alg: &str, data: &[u8]) -> Result<Vec<u8>, AuthError> {
    match alg {
        "sha256" => {
            let mut h = Sha256::new();
            h.update(data);
            Ok(h.finalize().to_vec())
        }
        "sha1" => {
            let mut h = Sha1::new();
            h.update(data);
            Ok(h.finalize().to_vec())
        }
        other => Err(AuthError::UnsupportedAlgorithm(other.into())),
    }
}

fn hmac_bytes(alg: &str, key: &[u8], data: &[u8]) -> Result<Vec<u8>, AuthError> {
    match alg {
        "sha256" => {
            let mut mac =
                HmacSha256::new_from_slice(key).map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        "sha1" => {
            let mut mac =
                HmacSha1::new_from_slice(key).map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        other => Err(AuthError::UnsupportedAlgorithm(other.into())),
    }
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn random_nonce_6() -> String {
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
