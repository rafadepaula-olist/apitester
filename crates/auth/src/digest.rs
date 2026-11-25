use std::collections::HashMap;

use apitester_core::models::auth::DigestConfig;
use md5::Md5;
use rand::RngCore;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{AuthApplication, AuthError, AuthRequestContext};

pub fn apply_digest(
    cfg: &DigestConfig,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    if cfg.nonce.is_empty() {
        return Err(AuthError::InvalidConfig(
            "digest nonce is empty; call apply_digest_challenge after receiving WWW-Authenticate"
                .into(),
        ));
    }
    build_digest_header(cfg, ctx)
}

pub fn apply_digest_challenge(
    cfg: &DigestConfig,
    ctx: &AuthRequestContext,
    www_authenticate: &str,
) -> Result<AuthApplication, AuthError> {
    let params = parse_digest_challenge(www_authenticate)?;
    let mut merged = cfg.clone();

    for (k, v) in &params {
        if v.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(AuthError::InvalidChallenge(format!(
                "WWW-Authenticate Digest parameter '{k}' contains control characters"
            )));
        }
    }

    if let Some(realm) = params.get("realm") {
        merged.realm = realm.clone();
    }
    if let Some(nonce) = params.get("nonce") {
        merged.nonce = nonce.clone();
    } else if merged.nonce.is_empty() {
        return Err(AuthError::InvalidChallenge(
            "WWW-Authenticate Digest header missing nonce".into(),
        ));
    }
    if let Some(alg) = params.get("algorithm") {
        let configured_sha256 = cfg.algorithm.to_ascii_uppercase().starts_with("SHA-256");
        let server_md5 = alg.to_ascii_uppercase().starts_with("MD5");
        if configured_sha256 && server_md5 {
            return Err(AuthError::InvalidChallenge(
                "refusing Digest algorithm downgrade from SHA-256 to MD5".into(),
            ));
        }
        merged.algorithm = alg.clone();
    }
    if let Some(opaque) = params.get("opaque") {
        merged.opaque = opaque.clone();
    }
    if let Some(qop_list) = params.get("qop") {
        let options: Vec<&str> = qop_list.split(',').map(|s| s.trim()).collect();
        if options.iter().any(|&o| o.eq_ignore_ascii_case("auth")) {
            merged.qop = "auth".into();
        } else if options.iter().any(|&o| o.eq_ignore_ascii_case("auth-int")) {
            merged.qop = "auth-int".into();
        } else if !qop_list.trim().is_empty() {
            return Err(AuthError::InvalidChallenge(format!(
                "unsupported Digest qop value: {qop_list}"
            )));
        }
    }

    build_digest_header(&merged, ctx)
}

fn build_digest_header(
    cfg: &DigestConfig,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    let parsed = Url::parse(&ctx.url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    let uri = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_string(),
    };
    let method = ctx.method.to_ascii_uppercase();

    let alg_raw = if cfg.algorithm.is_empty() {
        "MD5"
    } else {
        cfg.algorithm.as_str()
    };
    let alg_upper = alg_raw.to_ascii_uppercase();
    let (hash_fn, is_sess): (fn(&[u8]) -> String, bool) = match alg_upper.as_str() {
        "MD5" => (md5_hex, false),
        "MD5-SESS" => (md5_hex, true),
        "SHA-256" => (sha256_hex, false),
        "SHA-256-SESS" => (sha256_hex, true),
        other => {
            return Err(AuthError::UnsupportedAlgorithm(format!(
                "Digest algorithm {other} is not supported"
            )))
        }
    };

    let nc = if cfg.nonce_count.is_empty() {
        "00000001".to_string()
    } else {
        cfg.nonce_count.clone()
    };
    let cnonce = if cfg.client_nonce.is_empty() {
        random_hex_16()
    } else {
        cfg.client_nonce.clone()
    };

    let mut ha1 = hash_fn(format!("{}:{}:{}", cfg.username, cfg.realm, cfg.password).as_bytes());
    if is_sess {
        ha1 = hash_fn(format!("{ha1}:{}:{cnonce}", cfg.nonce).as_bytes());
    }

    let qop = cfg.qop.trim().to_ascii_lowercase();
    if !qop.is_empty() && qop != "auth" && qop != "auth-int" {
        return Err(AuthError::InvalidConfig(format!(
            "unsupported Digest qop value: {qop}"
        )));
    }
    let ha2 = if qop == "auth-int" {
        let body_hash = hash_fn(&ctx.body);
        hash_fn(format!("{method}:{uri}:{body_hash}").as_bytes())
    } else {
        hash_fn(format!("{method}:{uri}").as_bytes())
    };

    let response = if qop.is_empty() {
        hash_fn(format!("{ha1}:{}:{ha2}", cfg.nonce).as_bytes())
    } else {
        hash_fn(format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", cfg.nonce).as_bytes())
    };

    let mut parts = vec![
        format!("username=\"{}\"", escape_quoted(&cfg.username)),
        format!("realm=\"{}\"", escape_quoted(&cfg.realm)),
        format!("nonce=\"{}\"", escape_quoted(&cfg.nonce)),
        format!("uri=\"{}\"", escape_quoted(&uri)),
        format!("algorithm={alg_raw}"),
        format!("response=\"{response}\""),
    ];

    if !qop.is_empty() {
        parts.push(format!("qop={qop}"));
        parts.push(format!("nc={nc}"));
        parts.push(format!("cnonce=\"{}\"", escape_quoted(&cnonce)));
    }
    if !cfg.opaque.is_empty() {
        parts.push(format!("opaque=\"{}\"", escape_quoted(&cfg.opaque)));
    }

    Ok(AuthApplication::with_header(
        "Authorization",
        format!("Digest {}", parts.join(", ")),
    ))
}

fn parse_digest_challenge(header: &str) -> Result<HashMap<String, String>, AuthError> {
    let trimmed = header.trim();
    let rest = trimmed
        .strip_prefix("Digest")
        .or_else(|| trimmed.strip_prefix("digest"))
        .or_else(|| {
            if trimmed
                .get(..6)
                .is_some_and(|s| s.eq_ignore_ascii_case("digest"))
            {
                Some(&trimmed[6..])
            } else {
                None
            }
        })
        .ok_or_else(|| {
            AuthError::InvalidChallenge("WWW-Authenticate header does not start with Digest".into())
        })?;

    let mut map = HashMap::new();
    let bytes = rest.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b',' {
            i += 1;
        }
        let key = rest[key_start..i].trim().to_ascii_lowercase();
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1; // skip '='

        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let mut val = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                val.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            val
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            rest[val_start..i].trim().to_string()
        };

        if !key.is_empty() {
            map.insert(key, value);
        }
    }

    Ok(map)
}

fn escape_quoted(s: &str) -> String {
    s.chars()
        .filter(|&c| !c.is_control())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn random_hex_16() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
