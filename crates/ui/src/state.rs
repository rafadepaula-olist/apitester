use std::collections::HashMap;

use apitester_core::models::{
    ConsoleLogEntry, HttpMethod, ProtocolConfig, RequestItem, ResponseData,
};
use apitester_import_export::CodegenLanguage;
use apitester_net::{McpResource, McpTool, MqttClient, ProtoSchema, SocketIoClient, WsClient};
use apitester_proxy::CapturedExchange;
use apitester_runner::{
    CollectionRunReport, LoadProgressUpdate, LoadTestSummary, RunnerProgressEvent,
};
use chrono::{DateTime, Utc};
use tokio::task::AbortHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Collections,
    Environments,
    History,
    Cookies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSubTab {
    Params,
    Auth,
    Headers,
    Body,
    PreRequest,
    Tests,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseSubTab {
    Body,
    Headers,
    Cookies,
    TestResults,
    Console,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseBodyView {
    Pretty,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    PostmanCollection,
    PostmanEnvironment,
    Curl,
    OpenApi,
    Har,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerModalTab {
    CollectionRunner,
    LoadTester,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadProfileKind {
    Constant,
    Ramp,
    Spike,
    Peak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalState {
    None,
    ImportExport {
        format: ImportFormat,
        input_text: String,
        status: String,
    },
    CodeSnippet {
        language: CodegenLanguage,
        code: String,
    },
    CaptureProxy,
    RunnerAndLoadTester,
    Settings,
    CollectionEditor {
        collection_id: Uuid,
    },
}

/// Direction of a realtime protocol message log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogDirection {
    Sent,
    Received,
    System,
    Error,
}

#[derive(Debug, Clone)]
pub struct RealtimeLogEntry {
    pub timestamp: DateTime<Utc>,
    pub direction: LogDirection,
    pub summary: String,
    pub payload: String,
}

impl RealtimeLogEntry {
    pub fn new(
        direction: LogDirection,
        summary: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            direction,
            summary: summary.into(),
            payload: payload.into(),
        }
    }
}

/// State of one open workbench tab (HTTP, GraphQL, WebSocket, gRPC, MQTT, Socket.IO, MCP, SSE).
pub struct WorkbenchTab {
    pub id: Uuid,
    pub title: String,
    pub dirty: bool,
    pub collection_id: Option<Uuid>,
    pub request: RequestItem,
    pub response: Option<ResponseData>,
    pub in_flight: bool,
    pub cancel_handle: Option<AbortHandle>,
    pub req_sub_tab: RequestSubTab,
    pub resp_sub_tab: ResponseSubTab,
    pub resp_body_view: ResponseBodyView,
    pub resp_search: String,
    pub last_error: Option<String>,

    // GraphQL introspection output
    pub graphql_schema_json: Option<String>,

    // WebSocket state
    pub ws_connected: bool,
    pub ws_client: Option<WsClient>,
    pub ws_input: String,
    pub ws_log: Vec<RealtimeLogEntry>,

    // gRPC state
    pub grpc_proto_text: String,
    pub grpc_schema: Option<ProtoSchema>,
    pub grpc_payload: String,
    pub grpc_response: Option<String>,

    // MQTT state
    pub mqtt_connected: bool,
    pub mqtt_client: Option<MqttClient>,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_sub_topic: String,
    pub mqtt_pub_topic: String,
    pub mqtt_pub_payload: String,
    pub mqtt_log: Vec<RealtimeLogEntry>,

    // Socket.IO state
    pub sio_connected: bool,
    pub sio_client: Option<SocketIoClient>,
    pub sio_event_name: String,
    pub sio_payload: String,
    pub sio_log: Vec<RealtimeLogEntry>,

    // MCP state
    pub mcp_transport_http: bool,
    pub mcp_command: String,
    pub mcp_args: String,
    pub mcp_tools: Vec<McpTool>,
    pub mcp_resources: Vec<McpResource>,
    pub mcp_selected_tool: String,
    pub mcp_tool_args: String,
    pub mcp_output: String,

    // SSE state
    pub sse_connected: bool,
    pub sse_cancel: Option<AbortHandle>,
    pub sse_log: Vec<RealtimeLogEntry>,
}

impl WorkbenchTab {
    pub fn from_request(request: RequestItem, collection_id: Option<Uuid>) -> Self {
        let title = if request.name.trim().is_empty() {
            "Untitled Request".to_string()
        } else {
            request.name.clone()
        };
        Self {
            id: Uuid::new_v4(),
            title,
            dirty: false,
            collection_id,
            request,
            response: None,
            in_flight: false,
            cancel_handle: None,
            req_sub_tab: RequestSubTab::Params,
            resp_sub_tab: ResponseSubTab::Body,
            resp_body_view: ResponseBodyView::Pretty,
            resp_search: String::new(),
            last_error: None,
            graphql_schema_json: None,
            ws_connected: false,
            ws_client: None,
            ws_input: r#"{"type":"ping"}"#.to_string(),
            ws_log: Vec::new(),
            grpc_proto_text: String::new(),
            grpc_schema: None,
            grpc_payload: "{}".to_string(),
            grpc_response: None,
            mqtt_connected: false,
            mqtt_client: None,
            mqtt_host: "127.0.0.1".to_string(),
            mqtt_port: 1883,
            mqtt_sub_topic: "sensors/#".to_string(),
            mqtt_pub_topic: "sensors/temp".to_string(),
            mqtt_pub_payload: r#"{"value":22.5}"#.to_string(),
            mqtt_log: Vec::new(),
            sio_connected: false,
            sio_client: None,
            sio_event_name: "message".to_string(),
            sio_payload: r#"{"text":"hello"}"#.to_string(),
            sio_log: Vec::new(),
            mcp_transport_http: true,
            mcp_command: "npx".to_string(),
            mcp_args: String::new(),
            mcp_tools: Vec::new(),
            mcp_resources: Vec::new(),
            mcp_selected_tool: String::new(),
            mcp_tool_args: "{}".to_string(),
            mcp_output: String::new(),
            sse_connected: false,
            sse_cancel: None,
            sse_log: Vec::new(),
        }
    }

    pub fn new_protocol(protocol: ProtocolConfig) -> Self {
        let (name, url, method) = match &protocol {
            ProtocolConfig::Http => ("New HTTP Request", "https://httpbin.org/get", HttpMethod::Get),
            ProtocolConfig::GraphQL { .. } => (
                "New GraphQL Query",
                "https://countries.trevorblades.com/graphql",
                HttpMethod::Post,
            ),
            ProtocolConfig::WebSocket { .. } => {
                ("New WebSocket", "wss://echo.websocket.events", HttpMethod::Get)
            }
            ProtocolConfig::Grpc { .. } => {
                ("New gRPC Call", "http://127.0.0.1:50051", HttpMethod::Post)
            }
            ProtocolConfig::Mqtt { .. } => ("New MQTT Client", "mqtt://127.0.0.1:1883", HttpMethod::Get),
            ProtocolConfig::SocketIo { .. } => {
                ("New Socket.IO", "http://127.0.0.1:3000", HttpMethod::Get)
            }
            ProtocolConfig::Mcp { .. } => ("New MCP Client", "http://127.0.0.1:8080/mcp", HttpMethod::Post),
            ProtocolConfig::Sse { .. } => {
                ("New SSE Stream", "http://127.0.0.1:8080/events", HttpMethod::Get)
            }
        };
        let mut req = RequestItem::new(name, method, url);
        req.protocol = protocol;
        Self::from_request(req, None)
    }
}

/// Async messages delivered from background Tokio tasks back to the UI thread.
#[allow(clippy::large_enum_variant)]
pub enum UiMessage {
    HttpFinished {
        tab_id: Uuid,
        result: Result<ResponseData, String>,
        updated_globals: HashMap<String, String>,
        updated_env: HashMap<String, String>,
        updated_collection: (Option<Uuid>, HashMap<String, String>),
        extra_logs: Vec<ConsoleLogEntry>,
    },
    GraphQlIntrospected {
        tab_id: Uuid,
        result: Result<String, String>,
    },
    WsConnected {
        tab_id: Uuid,
        client: WsClient,
    },
    WsLog {
        tab_id: Uuid,
        entry: RealtimeLogEntry,
        disconnected: bool,
    },
    GrpcFinished {
        tab_id: Uuid,
        result: Result<String, String>,
    },
    MqttConnected {
        tab_id: Uuid,
        client: MqttClient,
    },
    MqttLog {
        tab_id: Uuid,
        entry: RealtimeLogEntry,
        disconnected: bool,
    },
    SocketIoConnected {
        tab_id: Uuid,
        client: SocketIoClient,
    },
    SocketIoLog {
        tab_id: Uuid,
        entry: RealtimeLogEntry,
        disconnected: bool,
    },
    McpOutput {
        tab_id: Uuid,
        output: String,
        tools: Option<Vec<McpTool>>,
        resources: Option<Vec<McpResource>>,
    },
    SseLog {
        tab_id: Uuid,
        entry: RealtimeLogEntry,
        ended: bool,
    },
    ProxyCaptured(CapturedExchange),
    RunnerProgress(RunnerProgressEvent),
    RunnerFinished(CollectionRunReport),
    LoadProgress(LoadProgressUpdate),
    LoadFinished(LoadTestSummary),
}
