use apitester_core::{HttpMethod, KeyValue, RawLanguage, RequestBody, RequestItem};
use serde::{Deserialize, Serialize};

/// A single HTTP request/response exchange captured by the local proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedExchange {
    pub id: String,
    pub timestamp_ms: i64,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Vec<u8>,
    pub status: u16,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Vec<u8>,
    pub duration_ms: u64,
}

impl CapturedExchange {
    /// Converts this captured exchange into an editable `RequestItem` that can be
    /// imported into a collection or opened in a request tab.
    pub fn to_request_item(&self) -> RequestItem {
        let method = self
            .method
            .parse::<HttpMethod>()
            .unwrap_or(HttpMethod::Get);

        let name = format!("{} {}", method.as_str(), short_url_label(&self.url));
        let mut item = RequestItem::new(name, method, self.url.clone());

        item.headers = self
            .request_headers
            .iter()
            .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
            .collect();

        item.body = self.build_request_body();
        item
    }

    fn build_request_body(&self) -> RequestBody {
        if self.request_body.is_empty() {
            return RequestBody::None;
        }

        let content_type = self
            .request_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_ascii_lowercase())
            .unwrap_or_default();

        if content_type.contains("application/x-www-form-urlencoded") {
            if let Ok(text) = std::str::from_utf8(&self.request_body) {
                let pairs = parse_form_urlencoded(text);
                if !pairs.is_empty() {
                    return RequestBody::UrlEncoded(pairs);
                }
            }
        }

        let language = if content_type.contains("json") {
            RawLanguage::Json
        } else if content_type.contains("xml") {
            RawLanguage::Xml
        } else if content_type.contains("html") {
            RawLanguage::Html
        } else if content_type.contains("javascript") {
            RawLanguage::Javascript
        } else {
            RawLanguage::Text
        };

        let content = String::from_utf8_lossy(&self.request_body).into_owned();
        RequestBody::Raw { content, language }
    }
}

fn short_url_label(url: &str) -> &str {
    if let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) {
        if let Some(idx) = rest.find('/') {
            return &rest[idx..];
        }
        return rest;
    }
    url
}

fn parse_form_urlencoded(input: &str) -> Vec<KeyValue> {
    input
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => KeyValue::new(percent_decode_form(k), percent_decode_form(v)),
            None => KeyValue::new(percent_decode_form(pair), ""),
        })
        .collect()
}

fn percent_decode_form(input: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_json_exchange_to_request_item() {
        let exchange = CapturedExchange {
            id: "ex-1".into(),
            timestamp_ms: 1_700_000_000_000,
            method: "POST".into(),
            url: "http://api.example.com/v1/users".into(),
            request_headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Trace".into(), "abc".into()),
            ],
            request_body: br#"{"name":"Ada"}"#.to_vec(),
            status: 201,
            response_headers: vec![("Content-Type".into(), "application/json".into())],
            response_body: br#"{"id":1}"#.to_vec(),
            duration_ms: 12,
        };

        let item = exchange.to_request_item();
        assert_eq!(item.name, "POST /v1/users");
        assert_eq!(item.method, HttpMethod::Post);
        assert_eq!(item.url, "http://api.example.com/v1/users");
        assert_eq!(item.headers.len(), 2);
        assert_eq!(item.headers[0].key, "Content-Type");
        assert_eq!(
            item.body,
            RequestBody::Raw {
                content: r#"{"name":"Ada"}"#.into(),
                language: RawLanguage::Json,
            }
        );
    }

    #[test]
    fn converts_empty_get_exchange_to_request_item() {
        let exchange = CapturedExchange {
            id: "ex-2".into(),
            timestamp_ms: 1_700_000_000_001,
            method: "GET".into(),
            url: "http://localhost:8080/health".into(),
            request_headers: vec![],
            request_body: vec![],
            status: 200,
            response_headers: vec![],
            response_body: b"ok".to_vec(),
            duration_ms: 2,
        };

        let item = exchange.to_request_item();
        assert_eq!(item.method, HttpMethod::Get);
        assert_eq!(item.body, RequestBody::None);
    }

    #[test]
    fn decodes_percent_encoded_form_urlencoded_without_double_encoding() {
        let exchange = CapturedExchange {
            id: "ex-3".into(),
            timestamp_ms: 1_700_000_000_002,
            method: "POST".into(),
            url: "http://localhost:8080/login".into(),
            request_headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            request_body: b"name=Ada+Lovelace&email=ada%40example.com".to_vec(),
            status: 200,
            response_headers: vec![],
            response_body: vec![],
            duration_ms: 1,
        };

        let item = exchange.to_request_item();
        assert_eq!(
            item.body,
            RequestBody::UrlEncoded(vec![
                KeyValue::new("name", "Ada Lovelace"),
                KeyValue::new("email", "ada@example.com"),
            ])
        );
    }
}
