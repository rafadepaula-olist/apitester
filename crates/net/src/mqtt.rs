use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

use crate::error::{NetError, NetResult};

/// Connection configuration for an MQTT v3.1.1 broker.
#[derive(Debug, Clone)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub keep_alive_secs: u64,
    pub clean_session: bool,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 1883,
            client_id: "apitester-client".to_string(),
            username: None,
            password: None,
            keep_alive_secs: 30,
            clean_session: true,
        }
    }
}

/// An incoming MQTT publish message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttIncomingMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// Lifecycle and message events from an MQTT connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MqttEvent {
    Connected,
    Message(MqttIncomingMessage),
    Disconnected,
    Error(String),
}

/// Async MQTT client backed by `rumqttc`.
#[derive(Clone)]
pub struct MqttClient {
    inner: AsyncClient,
}

impl MqttClient {
    /// Connects to an MQTT broker and spawns its background eventloop.
    pub fn connect(cfg: &MqttConfig) -> (Self, mpsc::Receiver<MqttEvent>) {
        let mut opts = MqttOptions::new(&cfg.client_id, &cfg.host, cfg.port);
        opts.set_keep_alive(Duration::from_secs(cfg.keep_alive_secs.max(5)));
        opts.set_clean_session(cfg.clean_session);
        if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
            opts.set_credentials(u, p);
        }

        let (client, mut eventloop) = AsyncClient::new(opts, 64);
        let (tx, rx) = mpsc::channel(128);

        tokio::spawn(async move {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        if tx.send(MqttEvent::Connected).await.is_err() {
                            break;
                        }
                    }
                    Ok(Event::Incoming(Packet::Publish(p))) => {
                        let msg = MqttIncomingMessage {
                            topic: p.topic,
                            payload: p.payload.to_vec(),
                            qos: qos_to_u8(p.qos),
                            retain: p.retain,
                        };
                        if tx.send(MqttEvent::Message(msg)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Event::Incoming(Packet::Disconnect)) => {
                        let _ = tx.send(MqttEvent::Disconnected).await;
                        break;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        let _ = tx.send(MqttEvent::Error(err.to_string())).await;
                        break;
                    }
                }
            }
        });

        (Self { inner: client }, rx)
    }

    pub async fn subscribe(&self, topic: &str, qos: u8) -> NetResult<()> {
        self.inner
            .subscribe(topic, u8_to_qos(qos))
            .await
            .map_err(|e| NetError::Mqtt(e.to_string()))
    }

    pub async fn unsubscribe(&self, topic: &str) -> NetResult<()> {
        self.inner
            .unsubscribe(topic)
            .await
            .map_err(|e| NetError::Mqtt(e.to_string()))
    }

    pub async fn publish(
        &self,
        topic: &str,
        qos: u8,
        retain: bool,
        payload: impl Into<Vec<u8>>,
    ) -> NetResult<()> {
        self.inner
            .publish(topic, u8_to_qos(qos), retain, payload.into())
            .await
            .map_err(|e| NetError::Mqtt(e.to_string()))
    }

    pub async fn disconnect(&self) -> NetResult<()> {
        self.inner
            .disconnect()
            .await
            .map_err(|e| NetError::Mqtt(e.to_string()))
    }
}

fn u8_to_qos(qos: u8) -> QoS {
    match qos {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        _ => QoS::ExactlyOnce,
    }
}

fn qos_to_u8(qos: QoS) -> u8 {
    match qos {
        QoS::AtMostOnce => 0,
        QoS::AtLeastOnce => 1,
        QoS::ExactlyOnce => 2,
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn mqtt_connect_receives_connack_and_publish_from_mock_broker() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let broker = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = sock.read(&mut buf).await.unwrap(); // CONNECT packet

            // Send CONNACK (0x20, remaining len 2, flags 0, return code 0)
            sock.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();

            // Send PUBLISH QoS 0: topic "sensors/temp", payload "22.5"
            let topic = b"sensors/temp";
            let payload = b"22.5";
            let mut pkt = vec![0x30, (2 + topic.len() + payload.len()) as u8];
            pkt.extend_from_slice(&(topic.len() as u16).to_be_bytes());
            pkt.extend_from_slice(topic);
            pkt.extend_from_slice(payload);
            sock.write_all(&pkt).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        });

        let cfg = MqttConfig {
            host: "127.0.0.1".into(),
            port: addr.port(),
            client_id: "test-client".into(),
            ..MqttConfig::default()
        };

        let (client, mut rx) = MqttClient::connect(&cfg);
        assert_eq!(rx.recv().await, Some(MqttEvent::Connected));
        assert_eq!(
            rx.recv().await,
            Some(MqttEvent::Message(MqttIncomingMessage {
                topic: "sensors/temp".into(),
                payload: b"22.5".to_vec(),
                qos: 0,
                retain: false,
            }))
        );
        let _ = client.disconnect().await;
        broker.await.unwrap();
    }
}
