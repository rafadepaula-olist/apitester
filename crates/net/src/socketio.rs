use serde_json::Value;
use tokio::sync::mpsc;

use crate::error::{NetError, NetResult};
use crate::websocket::{WsClient, WsEvent, WsMessage};

/// Events emitted by a Socket.IO v4 connection.
#[derive(Debug, Clone, PartialEq)]
pub enum SocketIoEvent {
    Connected {
        namespace: String,
        sid: Option<String>,
    },
    Event {
        namespace: String,
        name: String,
        data: Value,
    },
    Disconnected,
    Error(String),
}

/// Lightweight Socket.IO v4 client over WebSocket (Engine.IO v4 protocol).
#[derive(Debug, Clone)]
pub struct SocketIoClient {
    ws: WsClient,
    namespace: String,
}

impl SocketIoClient {
    /// Connects to a Socket.IO v4 server (`http://`, `https://`, `ws://`, or `wss://`)
    /// and joins `namespace` (defaults to `"/"`).
    pub async fn connect(
        base_url: &str,
        namespace: &str,
    ) -> NetResult<(Self, mpsc::Receiver<SocketIoEvent>)> {
        let ws_url = build_engineio_ws_url(base_url)?;
        let ns = normalize_namespace(namespace);
        let (ws, mut ws_rx) = WsClient::connect(&ws_url, &[], &[]).await?;

        let (tx, rx) = mpsc::channel(128);
        let ws_bg = ws.clone();
        let ns_bg = ns.clone();

        tokio::spawn(async move {
            while let Some(ev) = ws_rx.recv().await {
                match ev {
                    WsEvent::Connected => {}
                    WsEvent::Message(WsMessage::Text(packet)) => {
                        if let Some(rest) = packet.strip_prefix('0') {
                            // Engine.IO Open packet -> send Socket.IO Connect (`40` or `40/ns,`)
                            let _ = rest;
                            let connect_pkt = if ns_bg == "/" {
                                "40".to_string()
                            } else {
                                format!("40{ns_bg},")
                            };
                            if let Err(err) = ws_bg.send_text(connect_pkt).await {
                                let _ = tx.send(SocketIoEvent::Error(err.to_string())).await;
                                break;
                            }
                        } else if packet == "2" {
                            // Engine.IO Ping -> respond with Pong (`3`)
                            let _ = ws_bg.send_text("3").await;
                        } else if let Some(rest) = packet.strip_prefix("40") {
                            let (pkt_ns, payload) = split_namespace_payload(rest);
                            let sid = serde_json::from_str::<Value>(payload)
                                .ok()
                                .and_then(|v| v.get("sid").and_then(Value::as_str).map(str::to_string));
                            let _ = tx
                                .send(SocketIoEvent::Connected {
                                    namespace: pkt_ns,
                                    sid,
                                })
                                .await;
                        } else if let Some(rest) = packet.strip_prefix("42") {
                            let (pkt_ns, payload) = split_namespace_payload(rest);
                            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(payload) {
                                if let Some(name) = arr.first().and_then(Value::as_str) {
                                    let data = arr.get(1).cloned().unwrap_or(Value::Null);
                                    if tx
                                        .send(SocketIoEvent::Event {
                                            namespace: pkt_ns,
                                            name: name.to_string(),
                                            data,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        } else if packet.starts_with("41") {
                            let _ = tx.send(SocketIoEvent::Disconnected).await;
                            break;
                        }
                    }
                    WsEvent::Closed(_) => {
                        let _ = tx.send(SocketIoEvent::Disconnected).await;
                        break;
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(SocketIoEvent::Error(err)).await;
                        break;
                    }
                    WsEvent::Message(_) => {}
                }
            }
        });

        Ok((Self { ws, namespace: ns }, rx))
    }

    /// Emits a Socket.IO event (`42["event", payload]` or `42/ns,["event", payload]`).
    pub async fn emit(&self, event: &str, payload: Value) -> NetResult<()> {
        let body = serde_json::to_string(&serde_json::json!([event, payload]))?;
        let packet = if self.namespace == "/" {
            format!("42{body}")
        } else {
            format!("42{},{body}", self.namespace)
        };
        self.ws.send_text(packet).await
    }

    /// Gracefully disconnects from the Socket.IO namespace and closes the WebSocket.
    pub async fn disconnect(&self) -> NetResult<()> {
        let pkt = if self.namespace == "/" {
            "41".to_string()
        } else {
            format!("41{},", self.namespace)
        };
        let _ = self.ws.send_text(pkt).await;
        self.ws.close().await
    }
}

fn normalize_namespace(ns: &str) -> String {
    let trimmed = ns.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn split_namespace_payload(rest: &str) -> (String, &str) {
    if rest.starts_with('/') {
        if let Some((ns, payload)) = rest.split_once(',') {
            return (ns.to_string(), payload);
        }
    }
    ("/".to_string(), rest)
}

fn build_engineio_ws_url(base_url: &str) -> NetResult<String> {
    let mut url = url::Url::parse(base_url)?;
    match url.scheme() {
        "http" => {
            url.set_scheme("ws")
                .map_err(|()| NetError::SocketIo("failed to set ws scheme".into()))?;
        }
        "https" => {
            url.set_scheme("wss")
                .map_err(|()| NetError::SocketIo("failed to set wss scheme".into()))?;
        }
        "ws" | "wss" => {}
        other => {
            return Err(NetError::SocketIo(format!(
                "unsupported Socket.IO URL scheme: {other}"
            )));
        }
    }
    if url.path() == "/" || url.path().is_empty() {
        url.set_path("/socket.io/");
    }
    url.set_query(Some("EIO=4&transport=websocket"));
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    #[tokio::test]
    async fn socketio_v4_handshake_ping_pong_and_emit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

            // Engine.IO v4 Open
            ws.send(Message::Text(r#"0{"sid":"abc","pingInterval":25000,"pingTimeout":20000}"#.into()))
                .await
                .unwrap();

            // Expect Socket.IO connect `40`
            let connect = ws.next().await.unwrap().unwrap();
            assert_eq!(connect, Message::Text("40".into()));

            // Reply Socket.IO connect ack
            ws.send(Message::Text(r#"40{"sid":"sio-1"}"#.into()))
                .await
                .unwrap();

            // Send Engine.IO Ping `2`; client may send `3` (pong) and `42["chat",...]` in either order.
            ws.send(Message::Text("2".into())).await.unwrap();
            let mut got = Vec::new();
            for _ in 0..2 {
                if let Some(Ok(Message::Text(txt))) = ws.next().await {
                    got.push(txt);
                }
            }
            assert!(got.contains(&"3".to_string()), "expected pong, got {got:?}");
            assert!(
                got.contains(&r#"42["chat",{"msg":"hi"}]"#.to_string()),
                "expected chat emit, got {got:?}"
            );

            // Send server event back
            ws.send(Message::Text(r#"42["reply",{"ok":true}]"#.into()))
                .await
                .unwrap();
        });

        let (client, mut rx) = SocketIoClient::connect(&format!("http://{addr}"), "/")
            .await
            .unwrap();

        assert_eq!(
            rx.recv().await,
            Some(SocketIoEvent::Connected {
                namespace: "/".into(),
                sid: Some("sio-1".into()),
            })
        );

        client
            .emit("chat", serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();

        assert_eq!(
            rx.recv().await,
            Some(SocketIoEvent::Event {
                namespace: "/".into(),
                name: "reply".into(),
                data: serde_json::json!({"ok": true}),
            })
        );

        let _ = client.disconnect().await;
        server.await.unwrap();
    }
}
