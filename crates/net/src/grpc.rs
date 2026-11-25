use std::time::{Duration, Instant};

use apitester_core::models::{KeyValue, ResponseTimings};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};

use crate::error::{NetError, NetResult};

/// Parsed `.proto` field definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoField {
    pub field_type: String,
    pub name: String,
    pub tag: u32,
    pub repeated: bool,
}

/// Parsed `.proto` message definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoMessage {
    pub name: String,
    pub fields: Vec<ProtoField>,
}

/// Parsed `.proto` RPC method definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoMethod {
    pub name: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// Parsed `.proto` service definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoService {
    pub name: String,
    pub full_name: String,
    pub methods: Vec<ProtoMethod>,
}

/// Lightweight parsed representation of a `.proto` schema file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtoSchema {
    pub syntax: Option<String>,
    pub package: Option<String>,
    pub services: Vec<ProtoService>,
    pub messages: Vec<ProtoMessage>,
}

/// Parses a `.proto` file's syntax, package, services/RPCs, and messages without requiring `protoc`.
pub fn parse_proto(source: &str) -> ProtoSchema {
    let mut schema = ProtoSchema::default();
    let cleaned = strip_comments(source);

    let mut current_service: Option<ProtoService> = None;
    let mut current_message: Option<ProtoMessage> = None;

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if line == "}" {
            if let Some(svc) = current_service.take() {
                schema.services.push(svc);
            } else if let Some(msg) = current_message.take() {
                schema.messages.push(msg);
            }
            continue;
        }

        if let Some(svc) = current_service.as_mut() {
            if let Some(method) = parse_rpc_line(line) {
                svc.methods.push(method);
            }
            continue;
        }

        if let Some(msg) = current_message.as_mut() {
            if let Some(field) = parse_field_line(line) {
                msg.fields.push(field);
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("syntax") {
            if let Some(val) = rest.split('"').nth(1) {
                schema.syntax = Some(val.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("package ") {
            let pkg = rest.trim().trim_end_matches(';').trim();
            if !pkg.is_empty() {
                schema.package = Some(pkg.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("service ") {
            let name = rest.split(['{', ' ']).next().unwrap_or("").trim().to_string();
            let full_name = match &schema.package {
                Some(pkg) => format!("{pkg}.{name}"),
                None => name.clone(),
            };
            current_service = Some(ProtoService {
                name,
                full_name,
                methods: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("message ") {
            let name = rest.split(['{', ' ']).next().unwrap_or("").trim().to_string();
            current_message = Some(ProtoMessage {
                name,
                fields: Vec::new(),
            });
        }
    }

    schema
}

fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_rpc_line(line: &str) -> Option<ProtoMethod> {
    let rest = line.strip_prefix("rpc ")?.trim();
    let (name_part, after_name) = rest.split_once('(')?;
    let name = name_part.trim().to_string();

    let (req_part, after_req) = after_name.split_once(')')?;
    let (client_streaming, input_type) = parse_stream_type(req_part);

    let after_returns = after_req.split_once("returns")?.1;
    let (_, after_open) = after_returns.split_once('(')?;
    let (resp_part, _) = after_open.split_once(')')?;
    let (server_streaming, output_type) = parse_stream_type(resp_part);

    Some(ProtoMethod {
        name,
        input_type,
        output_type,
        client_streaming,
        server_streaming,
    })
}

fn parse_stream_type(s: &str) -> (bool, String) {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("stream ") {
        (true, rest.trim().to_string())
    } else {
        (false, trimmed.to_string())
    }
}

fn parse_field_line(line: &str) -> Option<ProtoField> {
    let line = line.trim_end_matches(';').trim();
    let (left, right) = line.split_once('=')?;
    let tag: u32 = right.split_whitespace().next()?.parse().ok()?;
    let tokens: Vec<&str> = left.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    let (repeated, field_type, name) = if tokens[0] == "repeated" && tokens.len() >= 3 {
        (true, tokens[1], tokens[2])
    } else if (tokens[0] == "optional" || tokens[0] == "required") && tokens.len() >= 3 {
        (false, tokens[1], tokens[2])
    } else {
        (false, tokens[0], tokens[1])
    };

    Some(ProtoField {
        field_type: field_type.to_string(),
        name: name.to_string(),
        tag,
        repeated,
    })
}

/// Encodes a payload into a gRPC length-prefixed frame (1-byte compression flag + 4-byte BE length).
pub fn encode_grpc_frame(payload: &[u8], compressed: bool) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(u8::from(compressed));
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Decodes a single gRPC length-prefixed frame into `(compressed, payload)`.
pub fn decode_grpc_frame(bytes: &[u8]) -> NetResult<(bool, Vec<u8>)> {
    if bytes.len() < 5 {
        return Err(NetError::Grpc(format!(
            "gRPC frame too short: {} bytes",
            bytes.len()
        )));
    }
    let compressed = bytes[0] != 0;
    let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    if bytes.len() < 5 + len {
        return Err(NetError::Grpc(format!(
            "incomplete gRPC frame: expected {len} bytes, got {}",
            bytes.len() - 5
        )));
    }
    Ok((compressed, bytes[5..5 + len].to_vec()))
}

/// Unary gRPC request descriptor.
#[derive(Debug, Clone)]
pub struct GrpcUnaryRequest {
    pub endpoint: String,
    pub service: String,
    pub method: String,
    pub payload: Vec<u8>,
    pub metadata: Vec<KeyValue>,
    pub timeout_ms: Option<u64>,
}

/// Unary gRPC response.
#[derive(Debug, Clone)]
pub struct GrpcUnaryResponse {
    pub grpc_status: i32,
    pub grpc_message: String,
    pub payload: Vec<u8>,
    pub headers: Vec<KeyValue>,
    pub timings: ResponseTimings,
}

fn is_valid_proto_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_valid_proto_service(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_valid_proto_ident)
}

/// Dispatches a unary gRPC call using HTTP/2 gRPC framing (`application/grpc`).
pub async fn call_unary(req: &GrpcUnaryRequest) -> NetResult<GrpcUnaryResponse> {
    let mut builder = reqwest::Client::builder();
    if req.endpoint.starts_with("http://") {
        builder = builder.http2_prior_knowledge();
    }
    if let Some(ms) = req.timeout_ms {
        builder = builder.timeout(Duration::from_millis(ms));
    }
    let client = builder.build()?;

    let service = req.service.trim_matches('/');
    let method = req.method.trim_matches('/');
    if !is_valid_proto_service(service) || !is_valid_proto_ident(method) {
        return Err(NetError::Grpc(format!(
            "invalid gRPC service/method identifier: '{service}/{method}'"
        )));
    }
    let base = req.endpoint.trim_end_matches('/');
    let url = format!("{base}/{service}/{method}");

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
    headers.insert("te", HeaderValue::from_static("trailers"));

    for kv in &req.metadata {
        if !kv.enabled || kv.key.trim().is_empty() {
            continue;
        }
        let name = HeaderName::from_bytes(kv.key.trim().as_bytes()).map_err(|e| {
            NetError::InvalidHeader {
                name: kv.key.clone(),
                reason: e.to_string(),
            }
        })?;
        let val = HeaderValue::from_str(&kv.value).map_err(|e| NetError::InvalidHeader {
            name: kv.key.clone(),
            reason: e.to_string(),
        })?;
        headers.append(name, val);
    }

    let frame = encode_grpc_frame(&req.payload, false);
    let started = Instant::now();
    let resp = client.post(&url).headers(headers).body(frame).send().await?;
    let ttfb_ms = started.elapsed().as_secs_f64() * 1000.0;

    let mut resp_headers = Vec::new();
    let mut grpc_status = 0_i32;
    let mut grpc_message = String::new();
    for (k, v) in resp.headers() {
        let val = v.to_str().unwrap_or("").to_string();
        if k.as_str().eq_ignore_ascii_case("grpc-status") {
            grpc_status = val.parse().unwrap_or(0);
        } else if k.as_str().eq_ignore_ascii_case("grpc-message") {
            grpc_message = val.clone();
        }
        resp_headers.push(KeyValue::new(k.as_str(), val));
    }

    let body = resp.bytes().await?;
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;

    let payload = if body.is_empty() {
        Vec::new()
    } else {
        decode_grpc_frame(&body)?.1
    };

    Ok(GrpcUnaryResponse {
        grpc_status,
        grpc_message,
        payload,
        headers: resp_headers,
        timings: ResponseTimings {
            ttfb_ms,
            total_ms: total_ms.max(ttfb_ms),
            ..ResponseTimings::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proto_extracts_service_rpcs_and_messages() {
        let proto = r#"
            syntax = "proto3";
            package acme.v1;

            // Greeter service
            service Greeter {
                rpc SayHello (HelloRequest) returns (HelloReply);
                rpc StreamReplies (HelloRequest) returns (stream HelloReply);
            }

            message HelloRequest {
                string name = 1;
                repeated string tags = 2;
            }
        "#;

        let schema = parse_proto(proto);
        assert_eq!(schema.syntax.as_deref(), Some("proto3"));
        assert_eq!(schema.package.as_deref(), Some("acme.v1"));
        assert_eq!(schema.services.len(), 1);
        assert_eq!(schema.services[0].full_name, "acme.v1.Greeter");
        assert_eq!(schema.services[0].methods.len(), 2);
        assert!(!schema.services[0].methods[0].server_streaming);
        assert!(schema.services[0].methods[1].server_streaming);
        assert_eq!(schema.messages.len(), 1);
        assert_eq!(schema.messages[0].fields.len(), 2);
        assert!(schema.messages[0].fields[1].repeated);
    }

    #[test]
    fn grpc_frame_encode_decode_round_trip() {
        let payload = b"\x0a\x05world";
        let frame = encode_grpc_frame(payload, false);
        assert_eq!(frame.len(), 5 + payload.len());
        let (compressed, decoded) = decode_grpc_frame(&frame).unwrap();
        assert!(!compressed);
        assert_eq!(decoded, payload);
    }
}
