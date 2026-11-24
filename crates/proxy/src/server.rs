use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytes::Bytes;
use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::capture::CapturedExchange;
use crate::error::ProxyError;

const BROADCAST_CAPACITY: usize = 256;

/// Local HTTP/HTTPS capture proxy server.
pub struct CaptureProxyServer {
    local_addr: SocketAddr,
    history: Arc<Mutex<Vec<CapturedExchange>>>,
    tx: broadcast::Sender<CapturedExchange>,
    shutdown_tx: watch::Sender<bool>,
    accept_task: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureProxyServer {
    /// Starts the proxy server bound to `bind_addr` (use `127.0.0.1:0` for an ephemeral port).
    pub async fn start(
        bind_addr: SocketAddr,
    ) -> Result<(Self, SocketAddr, broadcast::Receiver<CapturedExchange>), ProxyError> {
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        let history = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = broadcast::channel(BROADCAST_CAPACITY);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let mut connector = HttpConnector::new();
        connector.enforce_http(false);
        let client: Client<HttpConnector, Full<Bytes>> =
            Client::builder(TokioExecutor::new()).build(connector);

        let state = Arc::new(ProxyState {
            history: Arc::clone(&history),
            tx: tx.clone(),
            client,
        });

        let accept_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, _peer)) => {
                                let conn_state = Arc::clone(&state);
                                tokio::spawn(async move {
                                    let io = TokioIo::new(stream);
                                    let service = service_fn(move |req| {
                                        let req_state = Arc::clone(&conn_state);
                                        async move { handle_request(req, req_state).await }
                                    });
                                    let _ = http1::Builder::new()
                                        .preserve_header_case(true)
                                        .title_case_headers(true)
                                        .serve_connection(io, service)
                                        .with_upgrades()
                                        .await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        let server = Self {
            local_addr,
            history,
            tx,
            shutdown_tx,
            accept_task: Mutex::new(Some(accept_task)),
        };

        Ok((server, local_addr, rx))
    }

    /// Returns the socket address the proxy is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Creates a new broadcast receiver for live captured exchanges.
    pub fn subscribe(&self) -> broadcast::Receiver<CapturedExchange> {
        self.tx.subscribe()
    }

    /// Stops accepting new connections and shuts down the proxy accept loop.
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Ok(mut guard) = self.accept_task.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }

    /// Returns a snapshot of all captured exchanges recorded so far.
    pub fn captured_history(&self) -> Vec<CapturedExchange> {
        self.history
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Clears the in-memory captured history.
    pub fn clear_history(&self) {
        if let Ok(mut guard) = self.history.lock() {
            guard.clear();
        }
    }
}

impl Drop for CaptureProxyServer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ProxyState {
    history: Arc<Mutex<Vec<CapturedExchange>>>,
    tx: broadcast::Sender<CapturedExchange>,
    client: Client<HttpConnector, Full<Bytes>>,
}

impl ProxyState {
    fn record(&self, exchange: CapturedExchange) {
        if let Ok(mut guard) = self.history.lock() {
            guard.push(exchange.clone());
        }
        let _ = self.tx.send(exchange);
    }
}

async fn handle_request(
    req: Request<Incoming>,
    state: Arc<ProxyState>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    if req.method() == Method::CONNECT {
        return Ok(handle_connect(req, state).await);
    }
    Ok(handle_http_proxy(req, state).await)
}

async fn handle_connect(
    req: Request<Incoming>,
    state: Arc<ProxyState>,
) -> Response<Full<Bytes>> {
    let started = Instant::now();
    let Some(authority) = req.uri().authority().map(|a| a.to_string()) else {
        return text_response(StatusCode::BAD_REQUEST, "CONNECT requires host:port");
    };

    let request_headers = collect_headers(req.headers());

    let mut upstream = match TcpStream::connect(&authority).await {
        Ok(stream) => stream,
        Err(err) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                &format!("failed to connect to {authority}: {err}"),
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    state.record(CapturedExchange {
        id: Uuid::new_v4().to_string(),
        timestamp_ms: Utc::now().timestamp_millis(),
        method: "CONNECT".to_string(),
        url: format!("https://{authority}"),
        request_headers,
        request_body: Vec::new(),
        status: StatusCode::OK.as_u16(),
        response_headers: Vec::new(),
        response_body: Vec::new(),
        duration_ms,
    });

    tokio::spawn(async move {
        if let Ok(upgraded) = hyper::upgrade::on(req).await {
            let mut client_io = TokioIo::new(upgraded);
            let _ = copy_bidirectional(&mut client_io, &mut upstream).await;
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(Bytes::new()))
        .expect("valid 200 CONNECT response")
}

async fn handle_http_proxy(
    req: Request<Incoming>,
    state: Arc<ProxyState>,
) -> Response<Full<Bytes>> {
    let started = Instant::now();
    let method = req.method().clone();
    let request_headers = collect_headers(req.headers());

    let target_uri = match resolve_target_uri(&req) {
        Ok(uri) => uri,
        Err(msg) => return text_response(StatusCode::BAD_REQUEST, &msg),
    };
    let full_url = target_uri.to_string();

    let (parts, body) = req.into_parts();
    let req_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return text_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {err}"),
            );
        }
    };

    let mut forward_builder = Request::builder().method(method.clone()).uri(target_uri);
    for (name, value) in &parts.headers {
        if name.as_str().eq_ignore_ascii_case("proxy-connection") {
            continue;
        }
        forward_builder = forward_builder.header(name, value);
    }

    let forward_req = match forward_builder.body(Full::new(req_bytes.clone())) {
        Ok(r) => r,
        Err(err) => {
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to build upstream request: {err}"),
            );
        }
    };

    let upstream_resp = match state.client.request(forward_req).await {
        Ok(resp) => resp,
        Err(err) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                &format!("upstream request failed: {err}"),
            );
        }
    };

    let status = upstream_resp.status();
    let response_headers = collect_headers(upstream_resp.headers());
    let resp_headers_map = upstream_resp.headers().clone();

    let resp_bytes = match upstream_resp.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                &format!("failed to read upstream response body: {err}"),
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    state.record(CapturedExchange {
        id: Uuid::new_v4().to_string(),
        timestamp_ms: Utc::now().timestamp_millis(),
        method: method.as_str().to_string(),
        url: full_url,
        request_headers,
        request_body: req_bytes.to_vec(),
        status: status.as_u16(),
        response_headers,
        response_body: resp_bytes.to_vec(),
        duration_ms,
    });

    let mut client_resp = Response::builder().status(status);
    for (name, value) in &resp_headers_map {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        client_resp = client_resp.header(name, value);
    }

    client_resp
        .body(Full::new(resp_bytes))
        .unwrap_or_else(|_| text_response(StatusCode::INTERNAL_SERVER_ERROR, "response build error"))
}

fn resolve_target_uri(req: &Request<Incoming>) -> Result<Uri, String> {
    if req.uri().scheme().is_some() && req.uri().authority().is_some() {
        return Ok(req.uri().clone());
    }

    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| "missing absolute URI and Host header".to_string())?;

    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    format!("http://{host}{path_and_query}")
        .parse::<Uri>()
        .map_err(|e| format!("invalid target URI: {e}"))
}

fn collect_headers(headers: &hyper::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn is_hop_by_hop_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("transfer-encoding")
        || name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.eq_ignore_ascii_case("keep-alive")
        || name.eq_ignore_ascii_case("te")
        || name.eq_ignore_ascii_case("trailer")
        || name.eq_ignore_ascii_case("upgrade")
}

fn text_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body.to_owned())))
        .expect("valid text response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use apitester_core::{HttpMethod, RawLanguage, RequestBody};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn start_mock_http_server() -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("mock addr");

        let handle = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(|req: Request<Incoming>| async move {
                        let method = req.method().as_str().to_string();
                        let path = req.uri().to_string();
                        let body = req
                            .into_body()
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let reply = format!(
                            r#"{{"method":"{}","path":"{}","echo":{}}}"#,
                            method,
                            path,
                            String::from_utf8_lossy(&body)
                        );
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::CREATED)
                                .header("Content-Type", "application/json")
                                .header("X-Mock-Server", "apitester")
                                .body(Full::new(Bytes::from(reply)))
                                .unwrap(),
                        )
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (addr, handle)
    }

    #[tokio::test]
    async fn proxies_http_request_records_exchange_and_converts_to_request_item() {
        let (mock_addr, mock_handle) = start_mock_http_server().await;
        let (proxy, proxy_addr, mut rx) =
            CaptureProxyServer::start("127.0.0.1:0".parse().unwrap())
                .await
                .expect("start proxy");

        // Send a proxied HTTP/1.1 POST request directly to the proxy using absolute-form URI.
        let mut stream = TcpStream::connect(proxy_addr).await.expect("connect proxy");
        let target_url = format!("http://{mock_addr}/api/items?page=2");
        let payload = r#"{"item":"widget"}"#;
        let raw_request = format!(
            "POST {target_url} HTTP/1.1\r\n\
             Host: {mock_addr}\r\n\
             Content-Type: application/json\r\n\
             X-Client-Header: hello\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {payload}",
            payload.len()
        );
        stream
            .write_all(raw_request.as_bytes())
            .await
            .expect("write request");

        let mut raw_response = Vec::new();
        stream
            .read_to_end(&mut raw_response)
            .await
            .expect("read response");
        let resp_text = String::from_utf8_lossy(&raw_response);
        assert!(resp_text.starts_with("HTTP/1.1 201"));
        assert!(resp_text.contains(r#""method":"POST""#));
        assert!(resp_text.contains(r#""path":"/api/items?page=2""#));
        assert!(resp_text.contains(r#""echo":{"item":"widget"}"#));

        // Verify broadcast event and captured history.
        let captured = rx.recv().await.expect("broadcast exchange");
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.url, target_url);
        assert_eq!(captured.status, 201);
        assert_eq!(captured.request_body, payload.as_bytes());
        assert!(String::from_utf8_lossy(&captured.response_body).contains(r#""method":"POST""#));
        assert!(captured
            .response_headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-mock-server") && v == "apitester"));

        let history = proxy.captured_history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, captured.id);

        // Verify conversion to apitester-core RequestItem.
        let item = captured.to_request_item();
        assert_eq!(item.method, HttpMethod::Post);
        assert_eq!(item.url, target_url);
        assert!(item
            .headers
            .iter()
            .any(|h| h.key.eq_ignore_ascii_case("x-client-header") && h.value == "hello"));
        assert_eq!(
            item.body,
            RequestBody::Raw {
                content: payload.to_string(),
                language: RawLanguage::Json,
            }
        );

        proxy.clear_history();
        assert!(proxy.captured_history().is_empty());

        proxy.stop();
        mock_handle.abort();
    }

    #[tokio::test]
    async fn handles_connect_tunnel_bidirectionally() {
        // Start a simple TCP echo server to act as the CONNECT target.
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind echo");
        let echo_addr = echo_listener.local_addr().expect("echo addr");
        let echo_handle = tokio::spawn(async move {
            if let Ok((mut sock, _)) = echo_listener.accept().await {
                let mut buf = [0u8; 64];
                if let Ok(n) = sock.read(&mut buf).await {
                    let _ = sock.write_all(&buf[..n]).await;
                }
            }
        });

        let (proxy, proxy_addr, mut rx) =
            CaptureProxyServer::start("127.0.0.1:0".parse().unwrap())
                .await
                .expect("start proxy");

        let mut stream = TcpStream::connect(proxy_addr).await.expect("connect proxy");
        let connect_req = format!(
            "CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\n"
        );
        stream
            .write_all(connect_req.as_bytes())
            .await
            .expect("write connect");

        // Read the 200 OK response headers from the proxy.
        let mut resp_buf = vec![0u8; 1024];
        let mut filled = 0;
        loop {
            let n = stream.read(&mut resp_buf[filled..]).await.expect("read connect resp");
            assert!(n > 0, "unexpected EOF before CONNECT response");
            filled += n;
            if resp_buf[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let header_str = String::from_utf8_lossy(&resp_buf[..filled]);
        assert!(header_str.starts_with("HTTP/1.1 200"));

        // Send bytes through the established tunnel and confirm echo.
        stream.write_all(b"ping-tunnel").await.expect("write tunnel");
        let mut echoed = [0u8; 11];
        stream.read_exact(&mut echoed).await.expect("read echo");
        assert_eq!(&echoed, b"ping-tunnel");

        let captured = rx.recv().await.expect("connect captured");
        assert_eq!(captured.method, "CONNECT");
        assert_eq!(captured.status, 200);

        proxy.stop();
        echo_handle.abort();
    }
}
