use apitester_core::models::KeyValue;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{NetError, NetResult};

/// Outbound or inbound WebSocket frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Option<String>),
}

/// Live WebSocket lifecycle and message event delivered to callers/UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsEvent {
    Connected,
    Message(WsMessage),
    Closed(Option<String>),
    Error(String),
}

/// Async handle to an active WebSocket connection.
#[derive(Debug, Clone)]
pub struct WsClient {
    tx: mpsc::Sender<WsMessage>,
}

impl WsClient {
    /// Connects to a WebSocket (`ws://` or `wss://`) endpoint with optional custom headers
    /// and subprotocols, returning a `WsClient` handle and an inbound `WsEvent` receiver.
    pub async fn connect(
        url: &str,
        headers: &[KeyValue],
        subprotocols: &[String],
    ) -> NetResult<(Self, mpsc::Receiver<WsEvent>)> {
        let mut request = url
            .into_client_request()
            .map_err(|e| NetError::WebSocket(e.to_string()))?;

        let header_map = request.headers_mut();
        for kv in headers {
            if !kv.enabled || kv.key.trim().is_empty() {
                continue;
            }
            let name = HeaderName::from_bytes(kv.key.trim().as_bytes()).map_err(|e| {
                NetError::InvalidHeader {
                    name: kv.key.clone(),
                    reason: e.to_string(),
                }
            })?;
            let value = HeaderValue::from_str(&kv.value).map_err(|e| NetError::InvalidHeader {
                name: kv.key.clone(),
                reason: e.to_string(),
            })?;
            header_map.append(name, value);
        }

        if !subprotocols.is_empty() {
            let joined = subprotocols.join(", ");
            let value = HeaderValue::from_str(&joined).map_err(|e| NetError::InvalidHeader {
                name: "Sec-WebSocket-Protocol".into(),
                reason: e.to_string(),
            })?;
            header_map.insert("Sec-WebSocket-Protocol", value);
        }

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| NetError::WebSocket(e.to_string()))?;

        let (mut sink, mut stream) = ws_stream.split();
        let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(64);
        let (ev_tx, ev_rx) = mpsc::channel::<WsEvent>(128);

        let _ = ev_tx.send(WsEvent::Connected).await;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_out = out_rx.recv() => {
                        match maybe_out {
                            Some(msg) => {
                                let is_close = matches!(msg, WsMessage::Close(_));
                                let frame = to_tungstenite(msg);
                                if let Err(err) = sink.send(frame).await {
                                    let _ = ev_tx.send(WsEvent::Error(err.to_string())).await;
                                    break;
                                }
                                if is_close {
                                    let _ = ev_tx.send(WsEvent::Closed(None)).await;
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    maybe_in = stream.next() => {
                        match maybe_in {
                            Some(Ok(frame)) => {
                                match from_tungstenite(frame) {
                                    Some(WsMessage::Close(reason)) => {
                                        let _ = ev_tx.send(WsEvent::Closed(reason)).await;
                                        break;
                                    }
                                    Some(msg) => {
                                        if ev_tx.send(WsEvent::Message(msg)).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => {}
                                }
                            }
                            Some(Err(err)) => {
                                let _ = ev_tx.send(WsEvent::Error(err.to_string())).await;
                                break;
                            }
                            None => {
                                let _ = ev_tx.send(WsEvent::Closed(None)).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok((Self { tx: out_tx }, ev_rx))
    }

    pub async fn send(&self, msg: WsMessage) -> NetResult<()> {
        self.tx.send(msg).await.map_err(|_| NetError::ChannelClosed)
    }

    pub async fn send_text(&self, text: impl Into<String>) -> NetResult<()> {
        self.send(WsMessage::Text(text.into())).await
    }

    pub async fn close(&self) -> NetResult<()> {
        self.send(WsMessage::Close(None)).await
    }
}

fn to_tungstenite(msg: WsMessage) -> Message {
    match msg {
        WsMessage::Text(s) => Message::Text(s),
        WsMessage::Binary(b) => Message::Binary(b),
        WsMessage::Ping(b) => Message::Ping(b),
        WsMessage::Pong(b) => Message::Pong(b),
        WsMessage::Close(_) => Message::Close(None),
    }
}

fn from_tungstenite(msg: Message) -> Option<WsMessage> {
    match msg {
        Message::Text(s) => Some(WsMessage::Text(s)),
        Message::Binary(b) => Some(WsMessage::Binary(b)),
        Message::Ping(b) => Some(WsMessage::Ping(b)),
        Message::Pong(b) => Some(WsMessage::Pong(b)),
        Message::Close(frame) => Some(WsMessage::Close(
            frame.map(|f| f.reason.to_string()),
        )),
        Message::Frame(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn websocket_echo_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            if let Some(Ok(Message::Text(txt))) = ws.next().await {
                ws.send(Message::Text(format!("echo:{txt}"))).await.unwrap();
            }
            // Wait for client close frame before shutting down.
            let _ = ws.next().await;
            let _ = ws.close(None).await;
        });

        let (client, mut rx) = WsClient::connect(&format!("ws://{addr}"), &[], &[])
            .await
            .unwrap();

        assert_eq!(rx.recv().await, Some(WsEvent::Connected));
        client.send_text("hello").await.unwrap();
        assert_eq!(
            rx.recv().await,
            Some(WsEvent::Message(WsMessage::Text("echo:hello".into())))
        );
        client.close().await.unwrap();
        server.await.unwrap();
    }
}
