use apitester_core::models::auth::EdgeGridConfig;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::{AuthApplication, AuthError, AuthRequestContext};

type HmacSha256 = Hmac<Sha256>;

const MAX_BODY_SIZE: usize = 131_072;

pub fn apply_edgegrid(
    cfg: &EdgeGridConfig,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    if cfg.client_token.is_empty()
        || cfg.access_token.is_empty()
        || cfg.client_secret.is_empty()
    {
        return Err(AuthError::InvalidConfig(
            "EdgeGrid client_token, access_token, and client_secret must not be empty".into(),
        ));
    }

    let parsed = Url::parse(&ctx.url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed
        .host_str()
        .ok_or_else(|| AuthError::InvalidUrl("URL is missing host".into()))?
        .to_ascii_lowercase();
    let path_and_query = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_string(),
    };
    let method = ctx.method.to_ascii_uppercase();

    let timestamp = cfg
        .timestamp
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Utc::now().format("%Y%m%dT%H:%M:%S+0000").to_string());

    let nonce = if cfg.nonce.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        cfg.nonce.clone()
    };

    let auth_header_prefix = format!(
        "EG1-HMAC-SHA256 client_token={};access_token={};timestamp={timestamp};nonce={nonce};",
        cfg.client_token, cfg.access_token
    );

    let canonical_headers = build_canonical_headers(&cfg.headers_to_sign, &ctx.headers);

    let content_hash = if method == "POST" && !ctx.body.is_empty() {
        let slice = &ctx.body[..ctx.body.len().min(MAX_BODY_SIZE)];
        let mut hasher = Sha256::new();
        hasher.update(slice);
        STANDARD.encode(hasher.finalize())
    } else {
        String::new()
    };

    let signing_key = STANDARD.encode(hmac_sha256(
        cfg.client_secret.as_bytes(),
        timestamp.as_bytes(),
    )?);

    let data_to_sign = format!(
        "{method}\t{scheme}\t{host}\t{path_and_query}\t{canonical_headers}\t{content_hash}\t{auth_header_prefix}"
    );
    let signature = STANDARD.encode(hmac_sha256(
        signing_key.as_bytes(),
        data_to_sign.as_bytes(),
    )?);

    Ok(AuthApplication::with_header(
        "Authorization",
        format!("{auth_header_prefix}signature={signature}"),
    ))
}

fn build_canonical_headers(headers_to_sign: &str, request_headers: &[(String, String)]) -> String {
    if headers_to_sign.trim().is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for name in headers_to_sign.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some((_, val)) = request_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            let collapsed = val.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push_str(&name.to_ascii_lowercase());
            out.push(':');
            out.push_str(&collapsed);
            out.push('\t');
        }
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, AuthError> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|e| AuthError::Crypto(e.to_string()))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}
