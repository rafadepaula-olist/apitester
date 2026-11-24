use apitester_core::models::auth::{ApiKeyLocation, AwsV4Config};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{AuthApplication, AuthError, AuthRequestContext};

type HmacSha256 = Hmac<Sha256>;

/// RFC 3986 unreserved characters: A-Z a-z 0-9 - _ . ~
const AWS_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

const AWS_PATH_ENCODE_SET: &AsciiSet = &AWS_ENCODE_SET.remove(b'/');

pub fn apply_aws_v4(
    cfg: &AwsV4Config,
    ctx: &AuthRequestContext,
) -> Result<AuthApplication, AuthError> {
    apply_aws_v4_at(cfg, ctx, Utc::now())
}

pub(crate) fn apply_aws_v4_at(
    cfg: &AwsV4Config,
    ctx: &AuthRequestContext,
    now: DateTime<Utc>,
) -> Result<AuthApplication, AuthError> {
    if cfg.access_key.is_empty() || cfg.secret_key.is_empty() {
        return Err(AuthError::InvalidConfig(
            "AWS access_key and secret_key must not be empty".into(),
        ));
    }
    let region = if cfg.region.is_empty() {
        "us-east-1"
    } else {
        cfg.region.as_str()
    };
    let service = if cfg.service.is_empty() {
        "execute-api"
    } else {
        cfg.service.as_str()
    };

    let parsed = Url::parse(&ctx.url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
    let host = format_host(&parsed)?;

    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();
    let credential_scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let method = ctx.method.to_ascii_uppercase();
    let canonical_uri = build_canonical_uri(parsed.path());

    match cfg.add_to {
        ApiKeyLocation::Header => {
            let payload_hash = sha256_hex(&ctx.body);
            let mut canonical_headers: Vec<(String, String)> = ctx
                .headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), collapse_whitespace(v)))
                .filter(|(k, _)| {
                    k != "authorization" && k != "x-amz-date" && k != "x-amz-security-token"
                })
                .collect();

            if !canonical_headers.iter().any(|(k, _)| k == "host") {
                canonical_headers.push(("host".into(), host));
            }
            canonical_headers.push(("x-amz-date".into(), amz_date.clone()));
            if let Some(ref token) = cfg.session_token {
                if !token.is_empty() {
                    canonical_headers.push(("x-amz-security-token".into(), token.trim().into()));
                }
            }

            let (canonical_headers_str, signed_headers) =
                build_canonical_headers(&mut canonical_headers);
            let canonical_query = build_canonical_query_from_url(&parsed, &[]);

            let canonical_request = format!(
                "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers_str}\n{signed_headers}\n{payload_hash}"
            );
            let string_to_sign = format!(
                "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
                sha256_hex(canonical_request.as_bytes())
            );
            let signing_key = derive_signing_key(&cfg.secret_key, &date_stamp, region, service)?;
            let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);

            let auth_header = format!(
                "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
                cfg.access_key
            );

            let mut app = AuthApplication::default();
            app.headers.push(("X-Amz-Date".into(), amz_date));
            if let Some(ref token) = cfg.session_token {
                if !token.is_empty() {
                    app.headers
                        .push(("X-Amz-Security-Token".into(), token.clone()));
                }
            }
            app.headers.push(("Authorization".into(), auth_header));
            Ok(app)
        }
        ApiKeyLocation::Query => {
            let mut canonical_headers: Vec<(String, String)> = vec![("host".into(), host)];
            let (canonical_headers_str, signed_headers) =
                build_canonical_headers(&mut canonical_headers);

            let credential = format!("{}/{credential_scope}", cfg.access_key);
            let mut extra_params = vec![
                ("X-Amz-Algorithm".to_string(), "AWS4-HMAC-SHA256".to_string()),
                ("X-Amz-Credential".to_string(), credential),
                ("X-Amz-Date".to_string(), amz_date.clone()),
                ("X-Amz-Expires".to_string(), "86400".to_string()),
                ("X-Amz-SignedHeaders".to_string(), signed_headers.clone()),
            ];
            if let Some(ref token) = cfg.session_token {
                if !token.is_empty() {
                    extra_params.push(("X-Amz-Security-Token".to_string(), token.clone()));
                }
            }

            let canonical_query = build_canonical_query_from_url(&parsed, &extra_params);
            let payload_hash = if ctx.body.is_empty() {
                "UNSIGNED-PAYLOAD".to_string()
            } else {
                sha256_hex(&ctx.body)
            };

            let canonical_request = format!(
                "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers_str}\n{signed_headers}\n{payload_hash}"
            );
            let string_to_sign = format!(
                "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
                sha256_hex(canonical_request.as_bytes())
            );
            let signing_key = derive_signing_key(&cfg.secret_key, &date_stamp, region, service)?;
            let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes())?);

            let mut app = AuthApplication::default();
            app.query_params.extend(extra_params);
            app.query_params.push(("X-Amz-Signature".into(), signature));
            Ok(app)
        }
    }
}

fn format_host(url: &Url) -> Result<String, AuthError> {
    let host = url
        .host_str()
        .ok_or_else(|| AuthError::InvalidUrl("URL is missing host".into()))?;
    match url.port() {
        Some(port) => Ok(format!("{host}:{port}")),
        None => Ok(host.to_string()),
    }
}

fn build_canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    utf8_percent_encode(&decoded, AWS_PATH_ENCODE_SET).to_string()
}

fn build_canonical_query_from_url(url: &Url, extra: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (aws_encode(&k), aws_encode(&v)))
        .collect();
    for (k, v) in extra {
        pairs.push((aws_encode(k), aws_encode(v)));
    }
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn build_canonical_headers(headers: &mut Vec<(String, String)>) -> (String, String) {
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let mut merged: Vec<(String, String)> = Vec::new();
    for (k, v) in headers.drain(..) {
        if let Some(last) = merged.last_mut() {
            if last.0 == k {
                last.1.push(',');
                last.1.push_str(&v);
                continue;
            }
        }
        merged.push((k, v));
    }

    let mut canonical = String::new();
    let mut signed = Vec::with_capacity(merged.len());
    for (k, v) in &merged {
        canonical.push_str(k);
        canonical.push(':');
        canonical.push_str(v);
        canonical.push('\n');
        signed.push(k.as_str());
    }
    (canonical, signed.join(";"))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn aws_encode(input: &str) -> String {
    utf8_percent_encode(input, AWS_ENCODE_SET).to_string()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, AuthError> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|e| AuthError::Crypto(e.to_string()))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn derive_signing_key(
    secret: &str,
    date_stamp: &str,
    region: &str,
    service: &str,
) -> Result<Vec<u8>, AuthError> {
    let k_secret = format!("AWS4{secret}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date_stamp.as_bytes())?;
    let k_region = hmac_sha256(&k_date, region.as_bytes())?;
    let k_service = hmac_sha256(&k_region, service.as_bytes())?;
    hmac_sha256(&k_service, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Official AWS SigV4 documentation example (IAM ListUsers GET request).
    #[test]
    fn aws_sigv4_matches_official_test_vector() {
        let cfg = AwsV4Config {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
            service: "iam".into(),
            session_token: None,
            add_to: ApiKeyLocation::Header,
        };
        let ctx = AuthRequestContext {
            method: "GET".into(),
            url: "https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08".into(),
            headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded; charset=utf-8".into(),
            )],
            body: Vec::new(),
        };
        let fixed_time = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let app = apply_aws_v4_at(&cfg, &ctx, fixed_time).expect("aws v4 sign");

        let auth = app
            .headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.as_str())
            .expect("Authorization header");

        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }
}
