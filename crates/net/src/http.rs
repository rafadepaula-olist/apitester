use std::path::Path;
use std::time::{Duration, Instant};

use apitester_core::models::{
    AppSettings, ClientCertConfig, ConsoleLogEntry, ConsoleLogLevel, Cookie, FormFieldKind,
    KeyValue, ProxyConfig, ProxyType, RawLanguage, RequestBody, RequestItem, ResponseData,
    ResponseTimings,
};
use apitester_core::variables::{VariableContext, VariableResolver};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, COOKIE};
use reqwest::{Method, Proxy};
use tokio::sync::mpsc;
use url::Url;

use crate::error::{NetError, NetResult};

/// Execution options for an HTTP request (auth injections, cookies, client settings).
#[derive(Debug, Clone, Default)]
pub struct HttpRequestOptions {
    /// Additional headers (e.g., resolved from `apitester-auth`).
    pub extra_headers: Vec<(String, String)>,
    /// Additional query parameters (e.g., resolved from `apitester-auth`).
    pub extra_query: Vec<(String, String)>,
    /// Cookies to include in the outgoing `Cookie` header.
    pub cookies: Vec<Cookie>,
    /// Global/effective client settings (timeout, SSL verify, redirects, proxy, client certs).
    pub settings: AppSettings,
}

/// A parsed Server-Sent Events (SSE) message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
    pub retry: Option<u64>,
}

/// Async HTTP/1.1 + HTTP/2 client built on `reqwest` + `rustls`.
#[derive(Debug, Clone)]
pub struct HttpClient {
    shared_client: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        let shared_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(AppSettings::default().timeout_ms))
            .redirect(reqwest::redirect::Policy::limited(
                AppSettings::default().max_redirects as usize,
            ))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { shared_client }
    }

    fn client_for(&self, url: &Url, settings: &AppSettings) -> NetResult<reqwest::Client> {
        let defaults = AppSettings::default();
        let can_reuse = settings.verify_ssl == defaults.verify_ssl
            && settings.follow_redirects == defaults.follow_redirects
            && settings.max_redirects == defaults.max_redirects
            && settings.timeout_ms == defaults.timeout_ms
            && !settings.proxy.enabled
            && settings.client_certs.iter().all(|c| !c.enabled);
        if can_reuse {
            Ok(self.shared_client.clone())
        } else {
            build_reqwest_client(url, settings)
        }
    }

    /// Executes an HTTP request, measuring timings and capturing wire-level console logs.
    pub async fn execute(
        &self,
        req: &RequestItem,
        vars: &VariableContext,
        opts: &HttpRequestOptions,
    ) -> NetResult<ResponseData> {
        let resolver = VariableResolver::new(vars.clone());
        let mut logs = Vec::new();

        let url = build_resolved_url(req, &resolver, &opts.extra_query)?;
        let method = Method::from_bytes(req.method.as_str().as_bytes())
            .map_err(|_| NetError::InvalidMethod(req.method.as_str().to_string()))?;

        logs.push(log_entry(
            ConsoleLogLevel::Info,
            format!("> {} {}", method, url),
        ));

        let client = self.client_for(&url, &opts.settings)?;
        let headers = build_headers(req, &resolver, &opts.extra_headers, &opts.cookies)?;
        for (name, val) in &headers {
            logs.push(log_entry(
                ConsoleLogLevel::Log,
                format!("> {}: {}", name, val.to_str().unwrap_or("<binary>")),
            ));
        }

        // Measure DNS resolution time when host is a domain name.
        let dns_ms = measure_dns_ms(&url).await;

        let mut builder = client.request(method, url.clone()).headers(headers.clone());
        builder = apply_body(builder, &req.body, &resolver, &headers).await?;

        let started = Instant::now();
        let response = match builder.send().await {
            Ok(resp) => resp,
            Err(err) => {
                logs.push(log_entry(
                    ConsoleLogLevel::Error,
                    format!("Request failed: {err}"),
                ));
                return Err(NetError::Http(err));
            }
        };
        let ttfb_ms = started.elapsed().as_secs_f64() * 1000.0;

        let status = response.status();
        let status_code = status.as_u16();
        let status_text = status.canonical_reason().unwrap_or("").to_string();

        logs.push(log_entry(
            ConsoleLogLevel::Info,
            format!("< HTTP {} {}", status_code, status_text),
        ));

        let mut resp_headers = Vec::new();
        let mut resp_cookies = Vec::new();
        let host_for_cookies = url.host_str().unwrap_or("").to_string();

        for (name, value) in response.headers() {
            let val_str = value.to_str().unwrap_or("").to_string();
            logs.push(log_entry(
                ConsoleLogLevel::Log,
                format!("< {}: {}", name, val_str),
            ));
            if name.as_str().eq_ignore_ascii_case("set-cookie") {
                if let Some(cookie) = parse_set_cookie(&val_str, &host_for_cookies) {
                    resp_cookies.push(cookie);
                }
            }
            resp_headers.push(KeyValue::new(name.as_str(), val_str));
        }

        let body_bytes = response.bytes().await?.to_vec();
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        let size_bytes = body_bytes.len();

        let timings = ResponseTimings {
            dns_ms,
            tcp_ms: 0.0,
            tls_ms: 0.0,
            ttfb_ms,
            total_ms: total_ms.max(ttfb_ms),
        };

        Ok(ResponseData {
            status: status_code,
            status_text,
            headers: resp_headers,
            body: body_bytes,
            size_bytes,
            timings,
            cookies: resp_cookies,
            test_results: Vec::new(),
            console_logs: logs,
        })
    }

    /// Connects to a Server-Sent Events (SSE) endpoint and streams parsed `SseEvent` items
    /// over a `tokio::sync::mpsc::Receiver`.
    pub async fn stream_sse(
        &self,
        req: &RequestItem,
        vars: &VariableContext,
        opts: &HttpRequestOptions,
    ) -> NetResult<mpsc::Receiver<NetResult<SseEvent>>> {
        let resolver = VariableResolver::new(vars.clone());
        let url = build_resolved_url(req, &resolver, &opts.extra_query)?;
        let method = Method::from_bytes(req.method.as_str().as_bytes())
            .map_err(|_| NetError::InvalidMethod(req.method.as_str().to_string()))?;

        let client = self.client_for(&url, &opts.settings)?;
        let mut headers = build_headers(req, &resolver, &opts.extra_headers, &opts.cookies)?;
        if !headers.contains_key("accept") {
            headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        }

        let mut builder = client.request(method, url).headers(headers.clone());
        builder = apply_body(builder, &req.body, &resolver, &headers).await?;

        let response = builder.send().await?;
        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut current = SseEvent::default();
            let mut has_data = false;

            while let Some(chunk_res) = stream.next().await {
                match chunk_res {
                    Ok(chunk) => {
                        buffer.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(newline_idx) = buffer.find('\n') {
                            let mut line = buffer[..newline_idx].to_string();
                            buffer.drain(..=newline_idx);
                            if line.ends_with('\r') {
                                line.pop();
                            }

                            if line.is_empty() {
                                if has_data {
                                    let event = std::mem::take(&mut current);
                                    has_data = false;
                                    if tx.send(Ok(event)).await.is_err() {
                                        return;
                                    }
                                }
                            } else if line.starts_with(':') {
                                // SSE comment line; ignore.
                            } else {
                                let (field, value) = match line.split_once(':') {
                                    Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                                    None => (line.as_str(), ""),
                                };
                                match field {
                                    "data" => {
                                        if has_data {
                                            current.data.push('\n');
                                        }
                                        current.data.push_str(value);
                                        has_data = true;
                                    }
                                    "event" => current.event = Some(value.to_string()),
                                    "id" => current.id = Some(value.to_string()),
                                    "retry" => {
                                        if let Ok(ms) = value.parse::<u64>() {
                                            current.retry = Some(ms);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(NetError::Http(err))).await;
                        return;
                    }
                }
            }

            if has_data {
                let _ = tx.send(Ok(current)).await;
            }
        });

        Ok(rx)
    }
}

fn build_resolved_url(
    req: &RequestItem,
    resolver: &VariableResolver,
    extra_query: &[(String, String)],
) -> NetResult<Url> {
    let raw_url = resolver.resolve(&req.url);
    let mut url = Url::parse(&raw_url)?;

    {
        let mut pairs = url.query_pairs_mut();
        for kv in &req.params {
            if kv.enabled && !kv.key.is_empty() {
                pairs.append_pair(&resolver.resolve(&kv.key), &resolver.resolve(&kv.value));
            }
        }
        for (k, v) in extra_query {
            pairs.append_pair(k, v);
        }
    }

    // Remove trailing `?` if no query parameters were added.
    if url.query() == Some("") {
        url.set_query(None);
    }

    Ok(url)
}

fn build_headers(
    req: &RequestItem,
    resolver: &VariableResolver,
    extra_headers: &[(String, String)],
    cookies: &[Cookie],
) -> NetResult<HeaderMap> {
    let mut map = HeaderMap::new();

    for kv in &req.headers {
        if !kv.enabled || kv.key.trim().is_empty() {
            continue;
        }
        let name_str = resolver.resolve(&kv.key);
        let val_str = resolver.resolve(&kv.value);
        insert_header(&mut map, &name_str, &val_str)?;
    }

    for (k, v) in extra_headers {
        insert_header(&mut map, k, v)?;
    }

    if !cookies.is_empty() && !map.contains_key(COOKIE) {
        let cookie_header = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        insert_header(&mut map, "cookie", &cookie_header)?;
    }

    Ok(map)
}

fn insert_header(map: &mut HeaderMap, name: &str, value: &str) -> NetResult<()> {
    let header_name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|e| {
        NetError::InvalidHeader {
            name: name.to_string(),
            reason: e.to_string(),
        }
    })?;
    let header_value = HeaderValue::from_str(value).map_err(|e| NetError::InvalidHeader {
        name: name.to_string(),
        reason: e.to_string(),
    })?;
    map.append(header_name, header_value);
    Ok(())
}

fn build_reqwest_client(url: &Url, settings: &AppSettings) -> NetResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(!settings.verify_ssl)
        .cookie_store(true);

    if settings.timeout_ms > 0 {
        builder = builder.timeout(Duration::from_millis(settings.timeout_ms));
    }

    if settings.follow_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::limited(
            settings.max_redirects as usize,
        ));
    } else {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }

    if let Some(proxy) = build_proxy(&settings.proxy, url)? {
        builder = builder.proxy(proxy);
    }

    if let Some(host) = url.host_str() {
        for cert_cfg in &settings.client_certs {
            if cert_cfg.enabled && cert_matches_host(cert_cfg, host) {
                builder = apply_client_cert(builder, cert_cfg)?;
            }
        }
    }

    Ok(builder.build()?)
}

fn build_proxy(cfg: &ProxyConfig, url: &Url) -> NetResult<Option<Proxy>> {
    if !cfg.enabled || cfg.host.trim().is_empty() {
        return Ok(None);
    }

    if let Some(host) = url.host_str() {
        if cfg.bypass.iter().any(|b| b.eq_ignore_ascii_case(host)) {
            return Ok(None);
        }
    }

    let scheme = match cfg.proxy_type {
        ProxyType::Http => "http",
        ProxyType::Https => "https",
        ProxyType::Socks5 => "socks5",
    };
    let proxy_url = format!("{scheme}://{}:{}", cfg.host, cfg.port);
    let mut proxy = Proxy::all(&proxy_url)?;
    if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
        if !u.is_empty() {
            proxy = proxy.basic_auth(u, p);
        }
    }
    Ok(Some(proxy))
}

fn cert_matches_host(cfg: &ClientCertConfig, host: &str) -> bool {
    cfg.host.eq_ignore_ascii_case(host)
        || (cfg.host.starts_with("*.")
            && host
                .to_ascii_lowercase()
                .ends_with(&cfg.host[1..].to_ascii_lowercase()))
}

fn apply_client_cert(
    mut builder: reqwest::ClientBuilder,
    cfg: &ClientCertConfig,
) -> NetResult<reqwest::ClientBuilder> {
    if let Some(ca_path) = &cfg.ca_path {
        if !ca_path.trim().is_empty() {
            let ca_pem = std::fs::read(ca_path)?;
            let cert = reqwest::Certificate::from_pem(&ca_pem)?;
            builder = builder.add_root_certificate(cert);
        }
    }

    if !cfg.cert_path.trim().is_empty() && !cfg.key_path.trim().is_empty() {
        let mut combined = std::fs::read(&cfg.cert_path)?;
        combined.push(b'\n');
        combined.extend_from_slice(&std::fs::read(&cfg.key_path)?);
        let identity = reqwest::Identity::from_pem(&combined)?;
        builder = builder.identity(identity);
    }

    Ok(builder)
}

async fn apply_body(
    mut builder: reqwest::RequestBuilder,
    body: &RequestBody,
    resolver: &VariableResolver,
    headers: &HeaderMap,
) -> NetResult<reqwest::RequestBuilder> {
    let has_content_type = headers.contains_key(CONTENT_TYPE);

    match body {
        RequestBody::None => {}
        RequestBody::Raw { content, language } => {
            let resolved = resolver.resolve(content);
            if !has_content_type {
                let ct = match language {
                    RawLanguage::Json => "application/json",
                    RawLanguage::Xml => "application/xml",
                    RawLanguage::Html => "text/html",
                    RawLanguage::Javascript => "application/javascript",
                    RawLanguage::Text => "text/plain",
                };
                builder = builder.header(CONTENT_TYPE, ct);
            }
            builder = builder.body(resolved);
        }
        RequestBody::UrlEncoded(fields) => {
            let pairs: Vec<(String, String)> = fields
                .iter()
                .filter(|kv| kv.enabled && !kv.key.is_empty())
                .map(|kv| (resolver.resolve(&kv.key), resolver.resolve(&kv.value)))
                .collect();
            builder = builder.form(&pairs);
        }
        RequestBody::FormData(fields) => {
            let mut form = reqwest::multipart::Form::new();
            for field in fields {
                if !field.enabled || field.key.is_empty() {
                    continue;
                }
                let key = resolver.resolve(&field.key);
                match &field.kind {
                    FormFieldKind::Text { value } => {
                        form = form.text(key, resolver.resolve(value));
                    }
                    FormFieldKind::File { path, content_type } => {
                        let resolved_path = resolver.resolve(path);
                        let bytes = tokio::fs::read(&resolved_path).await?;
                        let filename = Path::new(&resolved_path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file")
                            .to_string();
                        let mut part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
                        if let Some(ct) = content_type {
                            if !ct.is_empty() {
                                part = part.mime_str(ct).map_err(|e| NetError::InvalidHeader {
                                    name: "content-type".into(),
                                    reason: e.to_string(),
                                })?;
                            }
                        }
                        form = form.part(key, part);
                    }
                }
            }
            builder = builder.multipart(form);
        }
        RequestBody::Binary { path } => {
            let resolved_path = resolver.resolve(path);
            let bytes = tokio::fs::read(&resolved_path).await?;
            if !has_content_type {
                builder = builder.header(CONTENT_TYPE, "application/octet-stream");
            }
            builder = builder.body(bytes);
        }
        RequestBody::GraphQL { query, variables } => {
            let resolved_query = resolver.resolve(query);
            let resolved_vars = resolver.resolve(variables);
            let vars_json: serde_json::Value = if resolved_vars.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&resolved_vars)?
            };
            let payload = serde_json::json!({
                "query": resolved_query,
                "variables": vars_json,
            });
            builder = builder.json(&payload);
        }
    }

    Ok(builder)
}

async fn measure_dns_ms(url: &Url) -> f64 {
    let Some(host) = url.host_str() else {
        return 0.0;
    };
    if host.parse::<std::net::IpAddr>().is_ok() {
        return 0.0;
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let started = Instant::now();
    if tokio::net::lookup_host((host, port)).await.is_ok() {
        started.elapsed().as_secs_f64() * 1000.0
    } else {
        0.0
    }
}

fn parse_set_cookie(header: &str, default_domain: &str) -> Option<Cookie> {
    let mut parts = header.split(';');
    let first = parts.next()?.trim();
    let (name, value) = first.split_once('=')?;

    let mut cookie = Cookie {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        domain: default_domain.to_string(),
        path: "/".to_string(),
        ..Cookie::default()
    };

    for attr in parts {
        let attr = attr.trim();
        if let Some((k, v)) = attr.split_once('=') {
            match k.trim().to_ascii_lowercase().as_str() {
                "domain" => cookie.domain = v.trim().to_string(),
                "path" => cookie.path = v.trim().to_string(),
                "samesite" => cookie.same_site = Some(v.trim().to_string()),
                _ => {}
            }
        } else {
            match attr.to_ascii_lowercase().as_str() {
                "httponly" => cookie.http_only = true,
                "secure" => cookie.secure = true,
                _ => {}
            }
        }
    }

    Some(cookie)
}

fn log_entry(level: ConsoleLogLevel, message: String) -> ConsoleLogEntry {
    ConsoleLogEntry {
        level,
        message,
        timestamp: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use apitester_core::models::{FormField, HttpMethod};
    use apitester_core::variables::VariableScope;
    use tempfile::NamedTempFile;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    async fn spawn_mock_http(response_raw: String) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap();
            let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
            socket.write_all(response_raw.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
            req_text
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn execute_raw_json_with_vars_auth_and_cookies() {
        let resp_raw = concat!(
            "HTTP/1.1 201 Created\r\n",
            "Content-Type: application/json\r\n",
            "Set-Cookie: sid=xyz789; Path=/; HttpOnly; SameSite=Lax\r\n",
            "Content-Length: 16\r\n",
            "Connection: close\r\n\r\n",
            "{\"created\":true}"
        )
        .to_string();
        let (base_url, server) = spawn_mock_http(resp_raw).await;

        let mut vars = VariableContext::new();
        vars.set(VariableScope::Environment, "base", &base_url);
        vars.set(VariableScope::Local, "user", "Ada");

        let mut req = RequestItem::new("Create", HttpMethod::Post, "{{base}}/users");
        req.params.push(KeyValue::new("active", "true"));
        req.body = RequestBody::Raw {
            content: r#"{"name":"{{user}}"}"#.into(),
            language: RawLanguage::Json,
        };

        let opts = HttpRequestOptions {
            extra_headers: vec![("Authorization".into(), "Bearer tok".into())],
            extra_query: vec![("api_key".into(), "k1".into())],
            ..HttpRequestOptions::default()
        };

        let res = HttpClient::new().execute(&req, &vars, &opts).await.unwrap();
        let received = server.await.unwrap();

        assert_eq!(res.status, 201);
        assert_eq!(res.body_text(), r#"{"created":true}"#);
        assert_eq!(res.cookies.len(), 1);
        assert_eq!(res.cookies[0].name, "sid");
        assert!(res.cookies[0].http_only);
        assert!(!res.console_logs.is_empty());
        assert!(received.contains("POST /users?active=true&api_key=k1"));
        assert!(received.contains("authorization: Bearer tok") || received.contains("Authorization: Bearer tok"));
        assert!(received.contains(r#"{"name":"Ada"}"#));
    }

    #[tokio::test]
    async fn execute_multipart_form_with_file_upload() {
        let resp_raw = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string();
        let (base_url, server) = spawn_mock_http(resp_raw).await;

        let mut tmp = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, b"hello-file").unwrap();

        let mut req = RequestItem::new("Upload", HttpMethod::Post, format!("{base_url}/upload"));
        req.body = RequestBody::FormData(vec![
            FormField::text("note", "attached"),
            FormField::file("doc", tmp.path().to_string_lossy().to_string()),
        ]);

        let res = HttpClient::new()
            .execute(&req, &VariableContext::new(), &HttpRequestOptions::default())
            .await
            .unwrap();
        let received = server.await.unwrap();

        assert_eq!(res.status, 200);
        assert!(received.contains("multipart/form-data"));
        assert!(received.contains("hello-file"));
    }

    #[tokio::test]
    async fn stream_sse_parses_events() {
        let sse_body = concat!(
            ": keep-alive\n",
            "id: 1\n",
            "event: message\n",
            "data: hello\n",
            "data: world\n\n",
            "id: 2\n",
            "retry: 3000\n",
            "data: done\n\n"
        );
        let resp_raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sse_body.len(),
            sse_body
        );
        let (base_url, _server) = spawn_mock_http(resp_raw).await;

        let req = RequestItem::new("Events", HttpMethod::Get, format!("{base_url}/events"));
        let mut rx = HttpClient::new()
            .stream_sse(&req, &VariableContext::new(), &HttpRequestOptions::default())
            .await
            .unwrap();

        let first = rx.recv().await.unwrap().unwrap();
        assert_eq!(first.id.as_deref(), Some("1"));
        assert_eq!(first.event.as_deref(), Some("message"));
        assert_eq!(first.data, "hello\nworld");

        let second = rx.recv().await.unwrap().unwrap();
        assert_eq!(second.id.as_deref(), Some("2"));
        assert_eq!(second.retry, Some(3000));
        assert_eq!(second.data, "done");
    }
}
