use apitester_core::models::{
    ApiKeyLocation, AuthConfig, FormField, FormFieldKind, KeyValue, RawLanguage, RequestBody,
    RequestItem,
};
use apitester_core::variables::{VariableContext, VariableResolver};
use url::Url;

/// Splits a raw URL into `(base_url_without_query, query_params)`.
///
/// Preserves `{{variable}}` placeholders and path templates that `url::Url` cannot parse directly.
pub(crate) fn split_url_and_query(raw: &str) -> (String, Vec<KeyValue>) {
    let trimmed = raw.trim();
    // Preserve fragment if any, split at first '?'
    let Some((base, rest)) = trimmed.split_once('?') else {
        return (trimmed.to_string(), Vec::new());
    };

    let (query_part, fragment) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };

    let mut params = Vec::new();
    if !query_part.is_empty() {
        for pair in query_part.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (percent_decode(k), percent_decode(v)),
                None => (percent_decode(pair), String::new()),
            };
            params.push(KeyValue::new(k, v));
        }
    }

    let base_url = match fragment {
        Some(f) => format!("{base}#{f}"),
        None => base.to_string(),
    };

    (base_url, params)
}

/// Strips the query string from a URL while preserving any `#fragment`.
pub(crate) fn strip_query(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.split_once('?') {
        Some((base, rest)) => match rest.split_once('#') {
            Some((_, frag)) => format!("{base}#{frag}"),
            None => base.to_string(),
        },
        None => trimmed.to_string(),
    }
}

/// Appends enabled query parameters onto a URL string.
pub(crate) fn build_url_with_params(base: &str, params: &[KeyValue]) -> String {
    let enabled: Vec<&KeyValue> = params
        .iter()
        .filter(|p| p.enabled && !p.key.is_empty())
        .collect();
    if enabled.is_empty() {
        return base.to_string();
    }

    let (without_frag, frag) = match base.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (base, None),
    };

    let mut out = without_frag.to_string();
    let mut has_query = without_frag.contains('?');
    for kv in enabled {
        out.push(if has_query { '&' } else { '?' });
        has_query = true;
        out.push_str(&encode_query_component(&kv.key));
        out.push('=');
        out.push_str(&encode_query_component(&kv.value));
    }

    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

/// Minimal percent-decoding for query strings (also decodes `+` as space).
pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Encodes a query parameter key or value while keeping `{{var}}` readable if unresolved.
pub(crate) fn encode_query_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push('%');
                out.push(char::from(HEX_UPPER[(b >> 4) as usize]));
                out.push(char::from(HEX_UPPER[(b & 0x0f) as usize]));
            }
        }
    }
    out
}

const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Infers `RawLanguage` from a `Content-Type` header value or body content.
pub(crate) fn infer_raw_language(content_type: Option<&str>, body: &str) -> RawLanguage {
    if let Some(ct) = content_type {
        let lower = ct.to_ascii_lowercase();
        if lower.contains("json") {
            return RawLanguage::Json;
        }
        if lower.contains("xml") {
            return RawLanguage::Xml;
        }
        if lower.contains("html") {
            return RawLanguage::Html;
        }
        if lower.contains("javascript") || lower.contains("ecmascript") {
            return RawLanguage::Javascript;
        }
        if lower.contains("text/plain") {
            return RawLanguage::Text;
        }
    }

    let trimmed = body.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return RawLanguage::Json;
    }
    if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
        return RawLanguage::Html;
    }
    if trimmed.starts_with("<?xml") {
        return RawLanguage::Xml;
    }
    RawLanguage::Text
}

/// Extracts a readable path suffix from a URL using `url::Url` when possible.
pub(crate) fn url_path_for_name(raw_url: &str) -> String {
    if let Ok(parsed) = Url::parse(raw_url) {
        let path = parsed.path();
        if !path.is_empty() && path != "/" {
            return path.to_string();
        }
        if let Some(host) = parsed.host_str() {
            return host.to_string();
        }
    }
    raw_url.to_string()
}

/// Resolved request fields ready for cURL export or code generation.
pub(crate) struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub basic_auth: Option<(String, String)>,
    pub body: RequestBody,
}

impl ResolvedRequest {
    pub(crate) fn from_request(req: &RequestItem, vars: &VariableContext) -> Self {
        let resolver = VariableResolver::new(vars.clone());
        let method = req.method.as_str().to_string();
        let base_url = resolver.resolve(&req.url);

        let mut params: Vec<KeyValue> = req
            .params
            .iter()
            .filter(|p| p.enabled && !p.key.is_empty())
            .map(|p| KeyValue::new(resolver.resolve(&p.key), resolver.resolve(&p.value)))
            .collect();

        let mut headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|h| h.enabled && !h.key.is_empty())
            .map(|h| (resolver.resolve(&h.key), resolver.resolve(&h.value)))
            .collect();

        let mut basic_auth = None;

        match &req.auth {
            AuthConfig::Bearer { token } => {
                let tok = resolver.resolve(token);
                if !tok.is_empty() && !has_header(&headers, "authorization") {
                    headers.push(("Authorization".to_string(), format!("Bearer {tok}")));
                }
            }
            AuthConfig::Basic { username, password } => {
                let u = resolver.resolve(username);
                let p = resolver.resolve(password);
                basic_auth = Some((u, p));
            }
            AuthConfig::ApiKey { key, value, add_to } => {
                let k = resolver.resolve(key);
                let v = resolver.resolve(value);
                if !k.is_empty() {
                    match add_to {
                        ApiKeyLocation::Header => {
                            if !has_header(&headers, &k) {
                                headers.push((k, v));
                            }
                        }
                        ApiKeyLocation::Query => {
                            params.push(KeyValue::new(k, v));
                        }
                    }
                }
            }
            AuthConfig::OAuth2(cfg) => {
                let tok = resolver.resolve(&cfg.access_token);
                if !tok.is_empty() {
                    match cfg.add_token_to {
                        ApiKeyLocation::Header => {
                            if !has_header(&headers, "authorization") {
                                let prefix = if cfg.header_prefix.is_empty() {
                                    "Bearer"
                                } else {
                                    cfg.header_prefix.as_str()
                                };
                                headers.push((
                                    "Authorization".to_string(),
                                    format!("{prefix} {tok}"),
                                ));
                            }
                        }
                        ApiKeyLocation::Query => {
                            params.push(KeyValue::new("access_token", tok));
                        }
                    }
                }
            }
            _ => {}
        }

        let url = build_url_with_params(&base_url, &params);

        let body = match &req.body {
            RequestBody::None => RequestBody::None,
            RequestBody::Raw { content, language } => RequestBody::Raw {
                content: resolver.resolve(content),
                language: *language,
            },
            RequestBody::UrlEncoded(kvs) => RequestBody::UrlEncoded(
                kvs.iter()
                    .filter(|kv| kv.enabled && !kv.key.is_empty())
                    .map(|kv| KeyValue::new(resolver.resolve(&kv.key), resolver.resolve(&kv.value)))
                    .collect(),
            ),
            RequestBody::FormData(fields) => RequestBody::FormData(
                fields
                    .iter()
                    .filter(|f| f.enabled && !f.key.is_empty())
                    .map(|f| FormField {
                        key: resolver.resolve(&f.key),
                        kind: match &f.kind {
                            FormFieldKind::Text { value } => FormFieldKind::Text {
                                value: resolver.resolve(value),
                            },
                            FormFieldKind::File { path, content_type } => FormFieldKind::File {
                                path: resolver.resolve(path),
                                content_type: content_type.clone(),
                            },
                        },
                        enabled: true,
                        description: f.description.clone(),
                    })
                    .collect(),
            ),
            RequestBody::Binary { path } => RequestBody::Binary {
                path: resolver.resolve(path),
            },
            RequestBody::GraphQL { query, variables } => RequestBody::GraphQL {
                query: resolver.resolve(query),
                variables: resolver.resolve(variables),
            },
        };

        Self {
            method,
            url,
            headers,
            basic_auth,
            body,
        }
    }
}

pub(crate) fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

/// Builds the JSON string payload for a GraphQL request body.
pub(crate) fn graphql_json_payload(query: &str, variables: &str) -> String {
    let vars_val: serde_json::Value = if variables.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(variables)
            .unwrap_or_else(|_| serde_json::Value::String(variables.to_string()))
    };
    serde_json::json!({
        "query": query,
        "variables": vars_val,
    })
    .to_string()
}

/// Standard base64 encoder (RFC 4648) for Basic auth header generation in code snippets.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
