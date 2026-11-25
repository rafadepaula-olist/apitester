use apitester_core::models::auth::OAuth1Config;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use rand::RngCore;
use sha1::Sha1;
use sha2::Sha256;
use url::Url;

use crate::{AuthApplication, AuthError, AuthRequestContext};

type HmacSha1 = Hmac<Sha1>;
type HmacSha256 = Hmac<Sha256>;

/// RFC 5849 Section 3.6 unreserved characters: A-Z a-z 0-9 - . _ ~
const OAUTH1_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub fn apply_oauth1(
    cfg: &OAuth1Config,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    if cfg.consumer_key.is_empty() {
        return Err(AuthError::InvalidConfig(
            "OAuth1 consumer_key must not be empty".into(),
        ));
    }

    let parsed = Url::parse(&ctx.url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    let base_url = build_base_url(&parsed)?;

    let sig_method = if cfg.signature_method.is_empty() {
        "HMAC-SHA1"
    } else {
        cfg.signature_method.as_str()
    };
    let version = if cfg.version.is_empty() {
        "1.0"
    } else {
        cfg.version.as_str()
    };
    let timestamp = cfg
        .timestamp
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| Utc::now().timestamp().to_string());
    let nonce = cfg
        .nonce
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(random_nonce);

    let mut oauth_params: Vec<(String, String)> = vec![
        ("oauth_consumer_key".into(), cfg.consumer_key.clone()),
        ("oauth_nonce".into(), nonce),
        ("oauth_signature_method".into(), sig_method.to_string()),
        ("oauth_timestamp".into(), timestamp),
        ("oauth_version".into(), version.to_string()),
    ];
    if !cfg.token.is_empty() || cfg.add_empty_params_to_sign {
        oauth_params.push(("oauth_token".into(), cfg.token.clone()));
    }

    // Collect all parameters for the signature base string.
    let mut all_params: Vec<(String, String)> = Vec::new();
    for (k, v) in parsed.query_pairs() {
        if !v.is_empty() || cfg.add_empty_params_to_sign {
            all_params.push((k.into_owned(), v.into_owned()));
        }
    }

    let is_form_urlencoded = ctx.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.to_ascii_lowercase()
                .starts_with("application/x-www-form-urlencoded")
    });
    if is_form_urlencoded && !ctx.body.is_empty() {
        for (k, v) in url::form_urlencoded::parse(&ctx.body) {
            if !v.is_empty() || cfg.add_empty_params_to_sign {
                all_params.push((k.into_owned(), v.into_owned()));
            }
        }
    }

    all_params.extend(oauth_params.iter().cloned());

    let mut encoded_params: Vec<(String, String)> = all_params
        .into_iter()
        .map(|(k, v)| (oauth_encode(&k), oauth_encode(&v)))
        .collect();
    encoded_params.sort();

    let normalized_params = encoded_params
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let method = ctx.method.to_ascii_uppercase();
    let base_string = format!(
        "{}&{}&{}",
        oauth_encode(&method),
        oauth_encode(&base_url),
        oauth_encode(&normalized_params)
    );
    let signing_key = format!(
        "{}&{}",
        oauth_encode(&cfg.consumer_secret),
        oauth_encode(&cfg.token_secret)
    );

    if sig_method.eq_ignore_ascii_case("PLAINTEXT") && parsed.scheme() != "https" {
        return Err(AuthError::InvalidConfig(
            "OAuth1 PLAINTEXT signature method requires an https:// URL".into(),
        ));
    }

    let signature = compute_signature(sig_method, &signing_key, &base_string)?;
    oauth_params.push(("oauth_signature".into(), signature));

    if cfg.add_params_to_header {
        let mut header_parts: Vec<String> = Vec::new();
        if let Some(ref realm) = cfg.realm {
            if !realm.is_empty() {
                header_parts.push(format!("realm=\"{}\"", realm.replace('"', "\\\"")));
            }
        }
        oauth_params.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in oauth_params {
            header_parts.push(format!("{}=\"{}\"", oauth_encode(&k), oauth_encode(&v)));
        }
        Ok(AuthApplication::with_header(
            "Authorization",
            format!("OAuth {}", header_parts.join(", ")),
        ))
    } else {
        Ok(AuthApplication {
            query_params: oauth_params,
            ..Default::default()
        })
    }
}

fn build_base_url(url: &Url) -> Result<String, AuthError> {
    let scheme = url.scheme().to_ascii_lowercase();
    let host = url
        .host_str()
        .ok_or_else(|| AuthError::InvalidUrl("URL is missing host".into()))?
        .to_ascii_lowercase();
    let port_part = match (scheme.as_str(), url.port()) {
        ("http", Some(80)) | ("https", Some(443)) | (_, None) => String::new(),
        (_, Some(p)) => format!(":{p}"),
    };
    Ok(format!("{scheme}://{host}{port_part}{}", url.path()))
}

fn compute_signature(
    method: &str,
    signing_key: &str,
    base_string: &str,
) -> Result<String, AuthError> {
    match method.to_ascii_uppercase().as_str() {
        "HMAC-SHA1" => {
            let mut mac = HmacSha1::new_from_slice(signing_key.as_bytes())
                .map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(base_string.as_bytes());
            Ok(STANDARD.encode(mac.finalize().into_bytes()))
        }
        "HMAC-SHA256" => {
            let mut mac = HmacSha256::new_from_slice(signing_key.as_bytes())
                .map_err(|e| AuthError::Crypto(e.to_string()))?;
            mac.update(base_string.as_bytes());
            Ok(STANDARD.encode(mac.finalize().into_bytes()))
        }
        "PLAINTEXT" => Ok(signing_key.to_string()),
        other => Err(AuthError::UnsupportedAlgorithm(format!(
            "OAuth1 signature method {other} is not supported"
        ))),
    }
}

fn oauth_encode(input: &str) -> String {
    utf8_percent_encode(input, OAUTH1_ENCODE_SET).to_string()
}

fn random_nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
