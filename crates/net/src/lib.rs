//! Async HTTP, GraphQL, WebSocket, SSE, gRPC, MQTT, MCP, and Socket.IO networking engine for apitester.

pub mod error;
pub mod graphql;
pub mod grpc;
pub mod http;
pub mod mcp;
pub mod mqtt;
pub mod socketio;
pub mod websocket;

pub use error::{NetError, NetResult};
pub use graphql::{execute_graphql, introspect_schema, GraphQlRequest, GraphQlResponse, INTROSPECTION_QUERY};
pub use grpc::{
    call_unary, decode_grpc_frame, encode_grpc_frame, parse_proto, GrpcUnaryRequest,
    GrpcUnaryResponse, ProtoField, ProtoMessage, ProtoMethod, ProtoSchema, ProtoService,
};
pub use http::{HttpClient, HttpRequestOptions, SseEvent};
pub use mcp::{McpClient, McpResource, McpTool};
pub use mqtt::{MqttClient, MqttConfig, MqttEvent, MqttIncomingMessage};
pub use socketio::{SocketIoClient, SocketIoEvent};
pub use websocket::{WsClient, WsEvent, WsMessage};
