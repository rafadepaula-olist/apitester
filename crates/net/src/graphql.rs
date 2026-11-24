use apitester_core::models::{HttpMethod, KeyValue, RawLanguage, RequestBody, RequestItem, ResponseData};
use apitester_core::variables::VariableContext;
use serde_json::{json, Value};

use crate::error::NetResult;
use crate::http::{HttpClient, HttpRequestOptions};

/// Standard GraphQL schema introspection query.
pub const INTROSPECTION_QUERY: &str = r#"
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      kind
      name
      description
      fields(includeDeprecated: true) {
        name
        description
        args {
          name
          description
          type { kind name ofType { kind name } }
          defaultValue
        }
        type { kind name ofType { kind name ofType { kind name } } }
        isDeprecated
        deprecationReason
      }
      inputFields {
        name
        description
        type { kind name ofType { kind name } }
        defaultValue
      }
      enumValues(includeDeprecated: true) {
        name
        description
        isDeprecated
      }
    }
  }
}
"#;

/// GraphQL execution request payload.
#[derive(Debug, Clone, Default)]
pub struct GraphQlRequest {
    pub url: String,
    pub query: String,
    pub variables: Option<Value>,
    pub operation_name: Option<String>,
    pub headers: Vec<KeyValue>,
}

/// Structured GraphQL execution response.
#[derive(Debug, Clone)]
pub struct GraphQlResponse {
    pub data: Option<Value>,
    pub errors: Vec<Value>,
    pub raw: ResponseData,
}

/// Executes a GraphQL query/mutation over HTTP POST and parses `data` and `errors`.
pub async fn execute_graphql(
    client: &HttpClient,
    req: &GraphQlRequest,
    vars: &VariableContext,
    opts: &HttpRequestOptions,
) -> NetResult<GraphQlResponse> {
    let mut payload = serde_json::Map::new();
    payload.insert("query".into(), Value::String(req.query.clone()));
    if let Some(v) = &req.variables {
        payload.insert("variables".into(), v.clone());
    }
    if let Some(op) = &req.operation_name {
        if !op.is_empty() {
            payload.insert("operationName".into(), Value::String(op.clone()));
        }
    }

    let mut http_req = RequestItem::new("GraphQL", HttpMethod::Post, req.url.clone());
    http_req.headers = req.headers.clone();
    http_req.body = RequestBody::Raw {
        content: Value::Object(payload).to_string(),
        language: RawLanguage::Json,
    };

    let raw = client.execute(&http_req, vars, opts).await?;
    let parsed: Value = serde_json::from_slice(&raw.body).unwrap_or_else(|_| json!({}));
    let data = parsed.get("data").cloned().filter(|v| !v.is_null());
    let errors = parsed
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(GraphQlResponse { data, errors, raw })
}

/// Runs the standard `__schema` introspection query against a GraphQL endpoint.
pub async fn introspect_schema(
    client: &HttpClient,
    url: &str,
    headers: &[KeyValue],
    vars: &VariableContext,
    opts: &HttpRequestOptions,
) -> NetResult<Value> {
    let req = GraphQlRequest {
        url: url.to_string(),
        query: INTROSPECTION_QUERY.to_string(),
        variables: None,
        operation_name: Some("IntrospectionQuery".to_string()),
        headers: headers.to_vec(),
    };
    let resp = execute_graphql(client, &req, vars, opts).await?;
    Ok(resp.data.unwrap_or_else(|| json!({})))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn graphql_query_and_introspection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(req_text.contains("__schema"));

            let body = r#"{"data":{"__schema":{"queryType":{"name":"Query"}}}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });

        let schema = introspect_schema(
            &HttpClient::new(),
            &format!("http://{addr}/graphql"),
            &[],
            &VariableContext::new(),
            &HttpRequestOptions::default(),
        )
        .await
        .unwrap();

        server.await.unwrap();
        assert_eq!(schema["__schema"]["queryType"]["name"], "Query");
    }
}
