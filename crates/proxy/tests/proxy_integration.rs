use std::net::SocketAddr;

use apitester_core::{HttpMethod, RequestBody};
use apitester_proxy::{CaptureProxyServer, CertificateAuthority};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start_simple_http_target() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind target");
    let addr = listener.local_addr().expect("target addr");

    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let body = "hello from target";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });

    (addr, handle)
}

#[tokio::test]
async fn integration_proxy_capture_and_ca_workflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cert_path = dir.path().join("root-ca.pem");
    let key_path = dir.path().join("root-ca-key.pem");

    let ca = CertificateAuthority::load_or_generate(&cert_path, &key_path).expect("ca");
    let leaf = ca.sign_leaf("localhost").expect("leaf");
    assert!(leaf.cert_pem.contains("BEGIN CERTIFICATE"));

    let (target_addr, target_handle) = start_simple_http_target().await;
    let (proxy, proxy_addr, mut rx) = CaptureProxyServer::start("127.0.0.1:0".parse().unwrap())
        .await
        .expect("start proxy");

    let mut conn = TcpStream::connect(proxy_addr).await.expect("connect proxy");
    let url = format!("http://{target_addr}/ping");
    let req = format!(
        "GET {url} HTTP/1.1\r\nHost: {target_addr}\r\nUser-Agent: apitester-test\r\nConnection: close\r\n\r\n"
    );
    conn.write_all(req.as_bytes()).await.expect("write");

    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.expect("read");
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(resp_str.starts_with("HTTP/1.1 200"));
    assert!(resp_str.ends_with("hello from target"));

    let exchange = rx.recv().await.expect("exchange event");
    assert_eq!(exchange.method, "GET");
    assert_eq!(exchange.url, url);
    assert_eq!(exchange.status, 200);
    assert_eq!(exchange.response_body, b"hello from target");

    let item = exchange.to_request_item();
    assert_eq!(item.method, HttpMethod::Get);
    assert_eq!(item.url, url);
    assert_eq!(item.body, RequestBody::None);
    assert!(item
        .headers
        .iter()
        .any(|h| h.key.eq_ignore_ascii_case("user-agent") && h.value == "apitester-test"));

    assert_eq!(proxy.captured_history().len(), 1);
    proxy.stop();
    target_handle.abort();
}
