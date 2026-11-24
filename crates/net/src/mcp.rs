use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use apitester_core::models::KeyValue;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::error::{NetError, NetResult};

/// An MCP tool descriptor returned by `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// An MCP resource descriptor returned by `resources/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "mimeType")]
    pub mime_type: Option<String>,
}

struct StdioTransport {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

enum McpTransport {
    Stdio(Box<Mutex<StdioTransport>>),
    Http {
        endpoint: String,
        headers: HeaderMap,
        client: reqwest::Client,
    },
}

/// Lightweight JSON-RPC 2.0 Model Context Protocol (MCP) client supporting Stdio and HTTP/SSE transports.
pub struct McpClient {
    transport: McpTransport,
    next_id: AtomicU64,
}

impl McpClient {
    /// Connects to an MCP server over child-process stdio (newline-delimited JSON-RPC 2.0).
    pub fn connect_stdio(command: &str, args: &[String]) -> NetResult<Self> {
        let mut child = Command::new(command)
            .args(args)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| NetError::Mcp("failed to open MCP child stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| NetError::Mcp("failed to open MCP child stdout".into()))?;

        Ok(Self {
            transport: McpTransport::Stdio(Box::new(Mutex::new(StdioTransport {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
            }))),
            next_id: AtomicU64::new(1),
        })
    }

    /// Connects to an MCP server over HTTP/SSE JSON-RPC 2.0 endpoint.
    pub fn connect_http(endpoint: impl Into<String>, headers: &[KeyValue]) -> NetResult<Self> {
        let mut map = HeaderMap::new();
        map.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        map.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );

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
            let val = HeaderValue::from_str(&kv.value).map_err(|e| NetError::InvalidHeader {
                name: kv.key.clone(),
                reason: e.to_string(),
            })?;
            map.append(name, val);
        }

        Ok(Self {
            transport: McpTransport::Http {
                endpoint: endpoint.into(),
                headers: map,
                client: reqwest::Client::new(),
            },
            next_id: AtomicU64::new(1),
        })
    }

    /// Sends a generic JSON-RPC 2.0 request and returns the `result` field.
    pub async fn request(&self, method: &str, params: Option<Value>) -> NetResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut req_obj = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if let Some(p) = params {
            req_obj["params"] = p;
        }

        let response: Value = match &self.transport {
            McpTransport::Stdio(mutex) => {
                let mut guard = mutex.lock().await;
                let mut line = serde_json::to_string(&req_obj)?;
                line.push('\n');
                guard.stdin.write_all(line.as_bytes()).await?;
                guard.stdin.flush().await?;

                let mut resp_line = String::new();
                let n = guard.stdout.read_line(&mut resp_line).await?;
                if n == 0 {
                    return Err(NetError::Mcp("MCP stdio server closed stdout".into()));
                }
                serde_json::from_str(&resp_line)?
            }
            McpTransport::Http {
                endpoint,
                headers,
                client,
            } => {
                let resp = client
                    .post(endpoint)
                    .headers(headers.clone())
                    .json(&req_obj)
                    .send()
                    .await?;
                let content_type = resp
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let body = resp.text().await?;
                if content_type.contains("text/event-stream") {
                    extract_sse_json(&body)?
                } else {
                    serde_json::from_str(&body)?
                }
            }
        };

        if let Some(err) = response.get("error") {
            return Err(NetError::Mcp(err.to_string()));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn initialize(&self, client_name: &str, client_version: &str) -> NetResult<Value> {
        self.request(
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": client_name,
                    "version": client_version
                }
            })),
        )
        .await
    }

    pub async fn list_tools(&self) -> NetResult<Vec<McpTool>> {
        let result = self.request("tools/list", None).await?;
        let tools_val = result.get("tools").cloned().unwrap_or_else(|| json!([]));
        Ok(serde_json::from_value(tools_val)?)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> NetResult<Value> {
        self.request(
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments
            })),
        )
        .await
    }

    pub async fn list_resources(&self) -> NetResult<Vec<McpResource>> {
        let result = self.request("resources/list", None).await?;
        let res_val = result
            .get("resources")
            .cloned()
            .unwrap_or_else(|| json!([]));
        Ok(serde_json::from_value(res_val)?)
    }
}

fn extract_sse_json(body: &str) -> NetResult<Value> {
    let mut data_lines = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim());
        }
    }
    let joined = data_lines.join("\n");
    serde_json::from_str(&joined).map_err(NetError::Json)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn mcp_http_initialize_list_tools_and_call_tool() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap();
                let text = String::from_utf8_lossy(&buf[..n]);
                let body_str = text.split("\r\n\r\n").nth(1).unwrap_or("{}");
                let req: Value = serde_json::from_str(body_str).unwrap();
                let id = req["id"].clone();
                let method = req["method"].as_str().unwrap_or("");

                let result = match method {
                    "initialize" => json!({ "protocolVersion": "2024-11-05", "serverInfo": { "name": "mock-mcp" } }),
                    "tools/list" => json!({ "tools": [{ "name": "ping", "description": "Ping tool", "inputSchema": {} }] }),
                    "tools/call" => json!({ "content": [{ "type": "text", "text": "pong" }] }),
                    _ => json!({}),
                };
                let resp_body = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
                let http_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                sock.write_all(http_resp.as_bytes()).await.unwrap();
            }
        });

        let client = McpClient::connect_http(format!("http://{addr}/mcp"), &[]).unwrap();
        let init = client.initialize("apitester", "0.1.0").await.unwrap();
        assert_eq!(init["serverInfo"]["name"], "mock-mcp");

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");

        let call = client.call_tool("ping", json!({})).await.unwrap();
        assert_eq!(call["content"][0]["text"], "pong");

        server.await.unwrap();
    }
}
