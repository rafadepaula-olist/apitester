use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AuthConfig;
use super::response::ResponseData;

/// HTTP method supporting standard verbs and custom extensions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    #[serde(untagged)]
    Custom(String),
}

impl HttpMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HttpMethod {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let method = match s.trim().to_ascii_uppercase().as_str() {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            "HEAD" => Self::Head,
            "OPTIONS" => Self::Options,
            other => Self::Custom(other.to_string()),
        };
        Ok(method)
    }
}

/// Key-value pair used for query params, headers, and URL-encoded bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub enabled: bool,
    pub description: String,
}

impl Default for KeyValue {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            enabled: true,
            description: String::new(),
        }
    }
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            enabled: true,
            description: String::new(),
        }
    }
}

/// Kind of multipart form-data field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FormFieldKind {
    Text {
        value: String,
    },
    File {
        path: String,
        #[serde(default)]
        content_type: Option<String>,
    },
}

/// A single field in a `multipart/form-data` request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormField {
    pub key: String,
    pub kind: FormFieldKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub description: String,
}

impl FormField {
    pub fn text(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            kind: FormFieldKind::Text {
                value: value.into(),
            },
            enabled: true,
            description: String::new(),
        }
    }

    pub fn file(key: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            kind: FormFieldKind::File {
                path: path.into(),
                content_type: None,
            },
            enabled: true,
            description: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Syntax language hint for raw request bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawLanguage {
    Text,
    #[default]
    Json,
    Xml,
    Html,
    Javascript,
}

/// Request payload configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", content = "data", rename_all = "snake_case")]
pub enum RequestBody {
    #[default]
    None,
    Raw {
        content: String,
        #[serde(default)]
        language: RawLanguage,
    },
    UrlEncoded(Vec<KeyValue>),
    FormData(Vec<FormField>),
    Binary {
        path: String,
    },
    GraphQL {
        query: String,
        #[serde(default)]
        variables: String,
    },
}

/// Protocol-specific configuration for a request item.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ProtocolConfig {
    #[default]
    Http,
    GraphQL {
        #[serde(default)]
        schema_url: Option<String>,
    },
    WebSocket {
        #[serde(default)]
        subprotocols: Vec<String>,
    },
    Grpc {
        #[serde(default)]
        proto_path: Option<String>,
        #[serde(default)]
        service: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        use_reflection: bool,
    },
    Mqtt {
        #[serde(default)]
        topic: String,
        #[serde(default)]
        qos: u8,
        #[serde(default)]
        client_id: String,
    },
    SocketIo {
        #[serde(default)]
        namespace: String,
        #[serde(default)]
        event: String,
        #[serde(default)]
        version: String,
    },
    Mcp {
        #[serde(default)]
        transport: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Vec<String>,
    },
    Sse {
        #[serde(default)]
        last_event_id: Option<String>,
    },
}

/// Pre-request and post-response JavaScript scripts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptConfig {
    pub pre_request: String,
    pub post_response: String,
}

/// A saved request/response example attached to a `RequestItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedExample {
    pub id: Uuid,
    pub name: String,
    pub request_url: String,
    pub request_method: HttpMethod,
    #[serde(default)]
    pub request_headers: Vec<KeyValue>,
    #[serde(default)]
    pub request_body: RequestBody,
    pub response: ResponseData,
}

/// An executable API request definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestItem {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub method: HttpMethod,
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body: RequestBody,
    pub auth: AuthConfig,
    pub protocol: ProtocolConfig,
    pub scripts: ScriptConfig,
    pub examples: Vec<SavedExample>,
}

impl Default for RequestItem {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "New Request".to_string(),
            description: String::new(),
            method: HttpMethod::Get,
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: RequestBody::None,
            auth: AuthConfig::Inherit,
            protocol: ProtocolConfig::Http,
            scripts: ScriptConfig::default(),
            examples: Vec::new(),
        }
    }
}

impl RequestItem {
    pub fn new(name: impl Into<String>, method: HttpMethod, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            method,
            url: url.into(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_method_parse_and_display() {
        assert_eq!("get".parse::<HttpMethod>().unwrap(), HttpMethod::Get);
        assert_eq!("POST".parse::<HttpMethod>().unwrap(), HttpMethod::Post);
        assert_eq!(
            "PURGE".parse::<HttpMethod>().unwrap(),
            HttpMethod::Custom("PURGE".into())
        );
        assert_eq!(HttpMethod::Patch.to_string(), "PATCH");
    }

    #[test]
    fn request_item_round_trip_json() {
        let mut req = RequestItem::new("Create User", HttpMethod::Post, "https://api.example.com/users");
        req.headers.push(KeyValue::new("Content-Type", "application/json"));
        req.body = RequestBody::Raw {
            content: r#"{"name":"Ada"}"#.into(),
            language: RawLanguage::Json,
        };
        req.scripts = ScriptConfig {
            pre_request: "console.log('pre');".into(),
            post_response: "pm.test('ok', () => {});".into(),
        };

        let json = serde_json::to_string(&req).expect("serialize");
        let back: RequestItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, req);
    }

    #[test]
    fn exhaustive_request_body_protocol_and_collection_json_store_round_trip() {
        use crate::models::{Collection, CollectionNode, Folder};
        use crate::storage::JsonStore;

        let bodies = vec![
            RequestBody::None,
            RequestBody::Raw {
                content: r#"{"hello":"world"}"#.into(),
                language: RawLanguage::Json,
            },
            RequestBody::UrlEncoded(vec![
                KeyValue::new("grant_type", "client_credentials"),
                KeyValue::new("scope", "read"),
            ]),
            RequestBody::FormData(vec![
                FormField::text("title", "Report"),
                FormField::file("attachment", "/tmp/report.pdf"),
            ]),
            RequestBody::Binary {
                path: "/tmp/payload.bin".into(),
            },
            RequestBody::GraphQL {
                query: "query { me { id } }".into(),
                variables: r#"{"id":1}"#.into(),
            },
        ];

        for body in &bodies {
            let json = serde_json::to_string_pretty(body).expect("serialize RequestBody");
            let back: RequestBody = serde_json::from_str(&json).expect("deserialize RequestBody");
            assert_eq!(&back, body);
        }

        let protocols = vec![
            ProtocolConfig::Http,
            ProtocolConfig::GraphQL {
                schema_url: Some("https://api.test/gql".into()),
            },
            ProtocolConfig::WebSocket {
                subprotocols: vec!["json".into()],
            },
            ProtocolConfig::Grpc {
                proto_path: Some("service.proto".into()),
                service: "acme.Greeter".into(),
                method: "SayHello".into(),
                use_reflection: false,
            },
            ProtocolConfig::Mqtt {
                topic: "sensors/#".into(),
                qos: 1,
                client_id: "c1".into(),
            },
            ProtocolConfig::SocketIo {
                namespace: "/chat".into(),
                event: "msg".into(),
                version: "4".into(),
            },
            ProtocolConfig::Mcp {
                transport: "http".into(),
                command: None,
                args: Vec::new(),
            },
            ProtocolConfig::Sse {
                last_event_id: Some("42".into()),
            },
        ];

        for proto in &protocols {
            let json = serde_json::to_string_pretty(proto).expect("serialize ProtocolConfig");
            let back: ProtocolConfig =
                serde_json::from_str(&json).expect("deserialize ProtocolConfig");
            assert_eq!(&back, proto);
        }

        // Save and reload a collection containing all body modes & protocols via JsonStore.
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStore::open(dir.path()).unwrap();
        let mut col = Collection::new("Exhaustive Collection");
        let mut folder = Folder::new("All Bodies");
        for (i, body) in bodies.into_iter().enumerate() {
            let mut req = RequestItem::new(
                format!("Req {i}"),
                HttpMethod::Post,
                "https://api.example.com/test",
            );
            req.body = body;
            req.protocol = protocols[i % protocols.len()].clone();
            folder.items.push(CollectionNode::Request(req));
        }
        col.items.push(CollectionNode::Folder(folder));

        store.save_collection(&col).expect("save_collection");
        let loaded = store.list_collections().expect("list_collections");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], col);
    }
}
