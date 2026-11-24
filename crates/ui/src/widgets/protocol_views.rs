use apitester_core::models::ProtocolConfig;
use apitester_core::variables::VariableContext;
use apitester_net::{
    call_unary, parse_proto, GrpcUnaryRequest, HttpClient, HttpRequestOptions, McpClient,
    MqttClient, MqttConfig, MqttEvent, SocketIoClient, SocketIoEvent, WsClient, WsEvent, WsMessage,
};
use egui::{Color32, RichText, ScrollArea, Ui};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;

use crate::state::{LogDirection, RealtimeLogEntry, UiMessage, WorkbenchTab};

/// Renders the dedicated workbench for non-HTTP/GraphQL protocols (WS, gRPC, MQTT, Socket.IO, MCP, SSE).
pub fn protocol_workbench_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
    vars: &VariableContext,
) {
    match tab.request.protocol.clone() {
        ProtocolConfig::WebSocket { subprotocols } => {
            websocket_ui(ui, tab, &subprotocols, rt, tx);
        }
        ProtocolConfig::Grpc { .. } => {
            grpc_ui(ui, tab, rt, tx);
        }
        ProtocolConfig::Mqtt { .. } => {
            mqtt_ui(ui, tab, rt, tx);
        }
        ProtocolConfig::SocketIo { namespace, .. } => {
            socketio_ui(ui, tab, &namespace, rt, tx);
        }
        ProtocolConfig::Mcp { .. } => {
            mcp_ui(ui, tab, rt, tx);
        }
        ProtocolConfig::Sse { .. } => {
            sse_ui(ui, tab, rt, tx, vars);
        }
        ProtocolConfig::Http | ProtocolConfig::GraphQL { .. } => {}
    }
}

fn websocket_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    subprotocols: &[String],
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("WS").strong().color(Color32::from_rgb(56, 189, 248)));
        ui.add(
            egui::TextEdit::singleline(&mut tab.request.url)
                .desired_width(ui.available_width() - 130.0)
                .hint_text("wss://echo.websocket.events"),
        );

        if tab.ws_connected {
            if ui.button("Disconnect").clicked() {
                if let Some(client) = tab.ws_client.take() {
                    rt.spawn(async move {
                        let _ = client.close().await;
                    });
                }
                tab.ws_connected = false;
            }
        } else if ui.button("Connect").clicked() {
            let url = tab.request.url.clone();
            let headers = tab.request.headers.clone();
            let subs = subprotocols.to_vec();
            let tab_id = tab.id;
            let tx = tx.clone();
            rt.spawn(async move {
                match WsClient::connect(&url, &headers, &subs).await {
                    Ok((client, mut rx)) => {
                        let _ = tx.send(UiMessage::WsConnected { tab_id, client });
                        while let Some(ev) = rx.recv().await {
                            let (entry, disconnected) = match ev {
                                WsEvent::Connected => (
                                    RealtimeLogEntry::new(LogDirection::System, "Connected", &url),
                                    false,
                                ),
                                WsEvent::Message(WsMessage::Text(t)) => (
                                    RealtimeLogEntry::new(LogDirection::Received, "Text", t),
                                    false,
                                ),
                                WsEvent::Message(WsMessage::Binary(b)) => (
                                    RealtimeLogEntry::new(
                                        LogDirection::Received,
                                        "Binary",
                                        format!("{} bytes", b.len()),
                                    ),
                                    false,
                                ),
                                WsEvent::Message(other) => (
                                    RealtimeLogEntry::new(
                                        LogDirection::System,
                                        "Frame",
                                        format!("{other:?}"),
                                    ),
                                    false,
                                ),
                                WsEvent::Closed(reason) => (
                                    RealtimeLogEntry::new(
                                        LogDirection::System,
                                        "Closed",
                                        reason.unwrap_or_default(),
                                    ),
                                    true,
                                ),
                                WsEvent::Error(err) => (
                                    RealtimeLogEntry::new(LogDirection::Error, "Error", err),
                                    true,
                                ),
                            };
                            let _ = tx.send(UiMessage::WsLog {
                                tab_id,
                                entry,
                                disconnected,
                            });
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(UiMessage::WsLog {
                            tab_id,
                            entry: RealtimeLogEntry::new(
                                LogDirection::Error,
                                "Connect Failed",
                                err.to_string(),
                            ),
                            disconnected: true,
                        });
                    }
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::multiline(&mut tab.ws_input)
                .desired_rows(3)
                .desired_width(ui.available_width() - 90.0)
                .code_editor(),
        );
        if ui
            .add_enabled(tab.ws_connected, egui::Button::new("Send"))
            .clicked()
        {
            let msg = tab.ws_input.clone();
            tab.ws_log
                .push(RealtimeLogEntry::new(LogDirection::Sent, "Text", &msg));
            if let Some(client) = tab.ws_client.clone() {
                rt.spawn(async move {
                    let _ = client.send_text(msg).await;
                });
            }
        }
    });

    ui.separator();
    realtime_log_ui(ui, "ws_log", &mut tab.ws_log);
}

fn grpc_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
) {
    let (mut service, mut method) = match &tab.request.protocol {
        ProtocolConfig::Grpc { service, method, .. } => (service.clone(), method.clone()),
        _ => (String::new(), String::new()),
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new("gRPC").strong().color(Color32::from_rgb(45, 212, 191)));
        ui.add(
            egui::TextEdit::singleline(&mut tab.request.url)
                .desired_width(280.0)
                .hint_text("http://127.0.0.1:50051"),
        );
        ui.label("Service:");
        ui.add(egui::TextEdit::singleline(&mut service).desired_width(160.0));
        ui.label("Method:");
        ui.add(egui::TextEdit::singleline(&mut method).desired_width(140.0));
        if ui.button("Invoke").clicked() {
            let req = GrpcUnaryRequest {
                endpoint: tab.request.url.clone(),
                service: service.clone(),
                method: method.clone(),
                payload: tab.grpc_payload.as_bytes().to_vec(),
                metadata: tab.request.headers.clone(),
                timeout_ms: Some(15_000),
            };
            let tab_id = tab.id;
            let tx = tx.clone();
            rt.spawn(async move {
                let res = call_unary(&req).await.map(|r| {
                    format!(
                        "gRPC Status: {} ({})\nTime: {:.1} ms\nPayload ({} bytes):\n{}",
                        r.grpc_status,
                        r.grpc_message,
                        r.timings.total_ms,
                        r.payload.len(),
                        String::from_utf8_lossy(&r.payload)
                    )
                });
                let _ = tx.send(UiMessage::GrpcFinished {
                    tab_id,
                    result: res.map_err(|e| e.to_string()),
                });
            });
        }
    });

    if let ProtocolConfig::Grpc {
        service: s,
        method: m,
        ..
    } = &mut tab.request.protocol
    {
        *s = service;
        *m = method;
    }

    ui.separator();
    ui.collapsing("Proto Schema Loader (.proto source)", |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut tab.grpc_proto_text)
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .code_editor()
                .hint_text("Paste .proto contents here and click Parse..."),
        );
        if ui.button("Parse .proto").clicked() {
            tab.grpc_schema = Some(parse_proto(&tab.grpc_proto_text));
        }
        if let Some(schema) = &tab.grpc_schema {
            for svc in &schema.services {
                ui.label(RichText::new(format!("Service: {}", svc.full_name)).strong());
                for m in &svc.methods {
                    if ui
                        .small_button(format!("{} ({}) -> ({})", m.name, m.input_type, m.output_type))
                        .clicked()
                    {
                        if let ProtocolConfig::Grpc {
                            service, method, ..
                        } = &mut tab.request.protocol
                        {
                            *service = svc.full_name.clone();
                            *method = m.name.clone();
                        }
                    }
                }
            }
        }
    });

    ui.separator();
    ui.label("Request Payload:");
    ui.add(
        egui::TextEdit::multiline(&mut tab.grpc_payload)
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .code_editor(),
    );

    if let Some(resp) = &tab.grpc_response {
        ui.separator();
        ui.label(RichText::new("Response:").strong());
        ScrollArea::vertical().show(ui, |ui| {
            ui.monospace(resp);
        });
    }
}

fn mqtt_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MQTT").strong().color(Color32::from_rgb(167, 139, 250)));
        ui.label("Host:");
        ui.add(egui::TextEdit::singleline(&mut tab.mqtt_host).desired_width(180.0));
        ui.label("Port:");
        ui.add(egui::DragValue::new(&mut tab.mqtt_port).range(1..=65535));

        if tab.mqtt_connected {
            if ui.button("Disconnect").clicked() {
                if let Some(client) = tab.mqtt_client.take() {
                    rt.spawn(async move {
                        let _ = client.disconnect().await;
                    });
                }
                tab.mqtt_connected = false;
            }
        } else if ui.button("Connect").clicked() {
            let cfg = MqttConfig {
                host: tab.mqtt_host.clone(),
                port: tab.mqtt_port,
                ..MqttConfig::default()
            };
            let tab_id = tab.id;
            let tx = tx.clone();
            rt.spawn(async move {
                let (client, mut rx) = MqttClient::connect(&cfg);
                let _ = tx.send(UiMessage::MqttConnected { tab_id, client });
                while let Some(ev) = rx.recv().await {
                    let (entry, disconnected) = match ev {
                        MqttEvent::Connected => (
                            RealtimeLogEntry::new(LogDirection::System, "Connected", &cfg.host),
                            false,
                        ),
                        MqttEvent::Message(m) => (
                            RealtimeLogEntry::new(
                                LogDirection::Received,
                                format!("Topic: {}", m.topic),
                                String::from_utf8_lossy(&m.payload).to_string(),
                            ),
                            false,
                        ),
                        MqttEvent::Disconnected => (
                            RealtimeLogEntry::new(LogDirection::System, "Disconnected", ""),
                            true,
                        ),
                        MqttEvent::Error(e) => (
                            RealtimeLogEntry::new(LogDirection::Error, "Error", e),
                            true,
                        ),
                    };
                    let _ = tx.send(UiMessage::MqttLog {
                        tab_id,
                        entry,
                        disconnected,
                    });
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Subscribe Topic:");
        ui.add(egui::TextEdit::singleline(&mut tab.mqtt_sub_topic).desired_width(220.0));
        if ui
            .add_enabled(tab.mqtt_connected, egui::Button::new("Subscribe"))
            .clicked()
        {
            let topic = tab.mqtt_sub_topic.clone();
            tab.mqtt_log
                .push(RealtimeLogEntry::new(LogDirection::System, "Subscribe", &topic));
            if let Some(c) = tab.mqtt_client.clone() {
                rt.spawn(async move {
                    let _ = c.subscribe(&topic, 0).await;
                });
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("Publish Topic:");
        ui.add(egui::TextEdit::singleline(&mut tab.mqtt_pub_topic).desired_width(180.0));
        ui.add(egui::TextEdit::singleline(&mut tab.mqtt_pub_payload).desired_width(240.0));
        if ui
            .add_enabled(tab.mqtt_connected, egui::Button::new("Publish"))
            .clicked()
        {
            let topic = tab.mqtt_pub_topic.clone();
            let payload = tab.mqtt_pub_payload.clone();
            tab.mqtt_log.push(RealtimeLogEntry::new(
                LogDirection::Sent,
                format!("Publish: {topic}"),
                &payload,
            ));
            if let Some(c) = tab.mqtt_client.clone() {
                rt.spawn(async move {
                    let _ = c.publish(&topic, 0, false, payload.into_bytes()).await;
                });
            }
        }
    });

    ui.separator();
    realtime_log_ui(ui, "mqtt_log", &mut tab.mqtt_log);
}

fn socketio_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    namespace: &str,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Socket.IO").strong().color(Color32::from_rgb(251, 191, 36)));
        ui.add(
            egui::TextEdit::singleline(&mut tab.request.url)
                .desired_width(ui.available_width() - 160.0)
                .hint_text("http://127.0.0.1:3000"),
        );
        if tab.sio_connected {
            if ui.button("Disconnect").clicked() {
                if let Some(c) = tab.sio_client.take() {
                    rt.spawn(async move {
                        let _ = c.disconnect().await;
                    });
                }
                tab.sio_connected = false;
            }
        } else if ui.button("Connect").clicked() {
            let url = tab.request.url.clone();
            let ns = if namespace.is_empty() {
                "/".to_string()
            } else {
                namespace.to_string()
            };
            let tab_id = tab.id;
            let tx = tx.clone();
            rt.spawn(async move {
                match SocketIoClient::connect(&url, &ns).await {
                    Ok((client, mut rx)) => {
                        let _ = tx.send(UiMessage::SocketIoConnected { tab_id, client });
                        while let Some(ev) = rx.recv().await {
                            let (entry, disconnected) = match ev {
                                SocketIoEvent::Connected { namespace, sid } => (
                                    RealtimeLogEntry::new(
                                        LogDirection::System,
                                        format!("Connected {namespace}"),
                                        sid.unwrap_or_default(),
                                    ),
                                    false,
                                ),
                                SocketIoEvent::Event { name, data, .. } => (
                                    RealtimeLogEntry::new(
                                        LogDirection::Received,
                                        name,
                                        data.to_string(),
                                    ),
                                    false,
                                ),
                                SocketIoEvent::Disconnected => (
                                    RealtimeLogEntry::new(LogDirection::System, "Disconnected", ""),
                                    true,
                                ),
                                SocketIoEvent::Error(e) => (
                                    RealtimeLogEntry::new(LogDirection::Error, "Error", e),
                                    true,
                                ),
                            };
                            let _ = tx.send(UiMessage::SocketIoLog {
                                tab_id,
                                entry,
                                disconnected,
                            });
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(UiMessage::SocketIoLog {
                            tab_id,
                            entry: RealtimeLogEntry::new(
                                LogDirection::Error,
                                "Connect Failed",
                                e.to_string(),
                            ),
                            disconnected: true,
                        });
                    }
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Event:");
        ui.add(egui::TextEdit::singleline(&mut tab.sio_event_name).desired_width(140.0));
        ui.label("JSON:");
        ui.add(egui::TextEdit::singleline(&mut tab.sio_payload).desired_width(260.0));
        if ui
            .add_enabled(tab.sio_connected, egui::Button::new("Emit"))
            .clicked()
        {
            let ev_name = tab.sio_event_name.clone();
            let raw_payload = tab.sio_payload.clone();
            let val = serde_json::from_str(&raw_payload)
                .unwrap_or_else(|_| serde_json::Value::String(raw_payload.clone()));
            tab.sio_log
                .push(RealtimeLogEntry::new(LogDirection::Sent, &ev_name, &raw_payload));
            if let Some(c) = tab.sio_client.clone() {
                rt.spawn(async move {
                    let _ = c.emit(&ev_name, val).await;
                });
            }
        }
    });

    ui.separator();
    realtime_log_ui(ui, "sio_log", &mut tab.sio_log);
}

fn mcp_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("MCP").strong().color(Color32::from_rgb(244, 114, 182)));
        ui.selectable_value(&mut tab.mcp_transport_http, true, "HTTP / SSE");
        ui.selectable_value(&mut tab.mcp_transport_http, false, "Stdio");
    });

    ui.horizontal(|ui| {
        if tab.mcp_transport_http {
            ui.label("Endpoint:");
            ui.add(
                egui::TextEdit::singleline(&mut tab.request.url)
                    .desired_width(360.0)
                    .hint_text("http://127.0.0.1:8080/mcp"),
            );
        } else {
            ui.label("Command:");
            ui.add(egui::TextEdit::singleline(&mut tab.mcp_command).desired_width(140.0));
            ui.label("Args:");
            ui.add(egui::TextEdit::singleline(&mut tab.mcp_args).desired_width(220.0));
        }

        if ui.button("Initialize & List Tools").clicked() {
            let is_http = tab.mcp_transport_http;
            let url = tab.request.url.clone();
            let headers = tab.request.headers.clone();
            let cmd = tab.mcp_command.clone();
            let args: Vec<String> = tab
                .mcp_args
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let tab_id = tab.id;
            let tx = tx.clone();

            rt.spawn(async move {
                let client_res = if is_http {
                    McpClient::connect_http(url, &headers)
                } else {
                    McpClient::connect_stdio(&cmd, &args)
                };
                match client_res {
                    Ok(client) => {
                        let init = client.initialize("apitester", "0.1.0").await;
                        let tools = client.list_tools().await.ok();
                        let resources = client.list_resources().await.ok();
                        let out = match init {
                            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
                            Err(e) => format!("Initialize error: {e}"),
                        };
                        let _ = tx.send(UiMessage::McpOutput {
                            tab_id,
                            output: out,
                            tools,
                            resources,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(UiMessage::McpOutput {
                            tab_id,
                            output: format!("Connect error: {e}"),
                            tools: None,
                            resources: None,
                        });
                    }
                }
            });
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Tool:");
        ui.add(egui::TextEdit::singleline(&mut tab.mcp_selected_tool).desired_width(160.0));
        ui.label("Args JSON:");
        ui.add(egui::TextEdit::singleline(&mut tab.mcp_tool_args).desired_width(240.0));
        if ui.button("Call Tool").clicked() {
            let is_http = tab.mcp_transport_http;
            let url = tab.request.url.clone();
            let headers = tab.request.headers.clone();
            let cmd = tab.mcp_command.clone();
            let args: Vec<String> = tab
                .mcp_args
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let tool = tab.mcp_selected_tool.clone();
            let tool_args = serde_json::from_str(&tab.mcp_tool_args)
                .unwrap_or_else(|_| serde_json::json!({}));
            let tab_id = tab.id;
            let tx = tx.clone();

            rt.spawn(async move {
                let client_res = if is_http {
                    McpClient::connect_http(url, &headers)
                } else {
                    McpClient::connect_stdio(&cmd, &args)
                };
                let out = match client_res {
                    Ok(client) => match client.call_tool(&tool, tool_args).await {
                        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(),
                        Err(e) => format!("Call error: {e}"),
                    },
                    Err(e) => format!("Connect error: {e}"),
                };
                let _ = tx.send(UiMessage::McpOutput {
                    tab_id,
                    output: out,
                    tools: None,
                    resources: None,
                });
            });
        }
    });

    if !tab.mcp_tools.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label("Discovered Tools:");
            for t in &tab.mcp_tools {
                if ui.small_button(&t.name).clicked() {
                    tab.mcp_selected_tool = t.name.clone();
                }
            }
        });
    }

    ui.separator();
    ScrollArea::vertical().show(ui, |ui| {
        ui.monospace(&tab.mcp_output);
    });
}

fn sse_ui(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    rt: &Handle,
    tx: &UnboundedSender<UiMessage>,
    vars: &VariableContext,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("SSE").strong().color(Color32::from_rgb(52, 211, 153)));
        ui.add(
            egui::TextEdit::singleline(&mut tab.request.url)
                .desired_width(ui.available_width() - 140.0)
                .hint_text("http://127.0.0.1:8080/events"),
        );
        if tab.sse_connected {
            if ui.button("Stop").clicked() {
                if let Some(handle) = tab.sse_cancel.take() {
                    handle.abort();
                }
                tab.sse_connected = false;
            }
        } else if ui.button("Stream").clicked() {
            let req = tab.request.clone();
            let vars = vars.clone();
            let tab_id = tab.id;
            let tx = tx.clone();
            tab.sse_connected = true;
            let handle = rt.spawn(async move {
                match HttpClient::new()
                    .stream_sse(&req, &vars, &HttpRequestOptions::default())
                    .await
                {
                    Ok(mut rx) => {
                        while let Some(item) = rx.recv().await {
                            match item {
                                Ok(ev) => {
                                    let _ = tx.send(UiMessage::SseLog {
                                        tab_id,
                                        entry: RealtimeLogEntry::new(
                                            LogDirection::Received,
                                            ev.event.unwrap_or_else(|| "message".into()),
                                            ev.data,
                                        ),
                                        ended: false,
                                    });
                                }
                                Err(err) => {
                                    let _ = tx.send(UiMessage::SseLog {
                                        tab_id,
                                        entry: RealtimeLogEntry::new(
                                            LogDirection::Error,
                                            "Stream Error",
                                            err.to_string(),
                                        ),
                                        ended: true,
                                    });
                                    return;
                                }
                            }
                        }
                        let _ = tx.send(UiMessage::SseLog {
                            tab_id,
                            entry: RealtimeLogEntry::new(LogDirection::System, "Stream Ended", ""),
                            ended: true,
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(UiMessage::SseLog {
                            tab_id,
                            entry: RealtimeLogEntry::new(
                                LogDirection::Error,
                                "Connect Failed",
                                err.to_string(),
                            ),
                            ended: true,
                        });
                    }
                }
            });
            tab.sse_cancel = Some(handle.abort_handle());
        }
    });

    ui.separator();
    realtime_log_ui(ui, "sse_log", &mut tab.sse_log);
}

fn realtime_log_ui(ui: &mut Ui, id: &str, entries: &mut Vec<RealtimeLogEntry>) {
    ui.horizontal(|ui| {
        ui.strong(format!("Messages ({})", entries.len()));
        if ui.small_button("Clear").clicked() {
            entries.clear();
        }
    });
    ScrollArea::vertical().id_salt(id).stick_to_bottom(true).show(ui, |ui| {
        for entry in entries.iter() {
            let (arrow, color) = match entry.direction {
                LogDirection::Sent => ("TX ->", Color32::from_rgb(251, 191, 36)),
                LogDirection::Received => ("RX <-", Color32::from_rgb(52, 211, 153)),
                LogDirection::System => ("SYS", Color32::from_rgb(96, 165, 250)),
                LogDirection::Error => ("ERR", Color32::from_rgb(248, 113, 113)),
            };
            ui.horizontal_wrapped(|ui| {
                ui.weak(entry.timestamp.format("%H:%M:%S").to_string());
                ui.label(RichText::new(arrow).strong().color(color));
                ui.strong(&entry.summary);
                ui.monospace(&entry.payload);
            });
        }
    });
}
