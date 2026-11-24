use std::collections::HashMap;
use std::str::FromStr;

use apitester_core::models::{
    ApiKeyLocation, AsapConfig, AuthConfig, AwsV4Config, Collection, CollectionNode, DigestConfig,
    EdgeGridConfig, Environment, Folder, FormField, FormFieldKind, HawkConfig, HttpMethod,
    JwtConfig, KeyValue, NtlmConfig, OAuth1Config, OAuth2Config, OAuth2GrantType, RawLanguage,
    RequestBody, RequestItem, ResponseData, SavedExample, ScriptConfig, Variable,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;
use uuid::Uuid;

use crate::error::{ImportExportError, ImportExportResult};
use crate::util::{build_url_with_params, infer_raw_language, split_url_and_query, strip_query};

const POSTMAN_V21_SCHEMA: &str =
    "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

/// Imports a Postman Collection v2.1.0 (and compatible v2.0) JSON document into a `Collection`.
pub fn import_postman_collection(json_str: &str) -> ImportExportResult<Collection> {
    let root: Value = serde_json::from_str(json_str)?;
    let obj = root
        .as_object()
        .ok_or_else(|| ImportExportError::InvalidFormat("expected JSON object".into()))?;

    let info = obj
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| ImportExportError::InvalidFormat("missing Postman `info` object".into()))?;

    let id = info
        .get("_postman_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4);

    let name = info
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Imported Collection")
        .to_string();

    let description = extract_description(info.get("description"));

    let items = obj
        .get("item")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_item).collect())
        .unwrap_or_default();

    let variables = obj
        .get("variable")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_variable).collect())
        .unwrap_or_default();

    let auth = match obj.get("auth") {
        Some(v) if !v.is_null() => parse_auth(v),
        _ => AuthConfig::NoAuth,
    };

    let scripts = parse_events(obj.get("event"));
    let now = Utc::now();

    Ok(Collection {
        id,
        name,
        description,
        items,
        variables,
        auth,
        scripts,
        created_at: now,
        updated_at: now,
    })
}

/// Exports a `Collection` as a Postman Collection v2.1.0 JSON string.
pub fn export_postman_collection(collection: &Collection) -> ImportExportResult<String> {
    let mut root = Map::new();
    let mut info = Map::new();
    info.insert("_postman_id".into(), Value::String(collection.id.to_string()));
    info.insert("name".into(), Value::String(collection.name.clone()));
    if !collection.description.is_empty() {
        info.insert(
            "description".into(),
            Value::String(collection.description.clone()),
        );
    }
    info.insert("schema".into(), Value::String(POSTMAN_V21_SCHEMA.into()));
    root.insert("info".into(), Value::Object(info));

    let items: Vec<Value> = collection.items.iter().map(export_node).collect();
    root.insert("item".into(), Value::Array(items));

    if let Some(auth_val) = export_auth(&collection.auth) {
        root.insert("auth".into(), auth_val);
    }

    let events = export_events(&collection.scripts);
    if !events.is_empty() {
        root.insert("event".into(), Value::Array(events));
    }

    if !collection.variables.is_empty() {
        let vars: Vec<Value> = collection.variables.iter().map(export_variable).collect();
        root.insert("variable".into(), Value::Array(vars));
    }

    Ok(serde_json::to_string_pretty(&Value::Object(root))?)
}

/// Imports a Postman Environment JSON document into an `Environment`.
pub fn import_postman_environment(json_str: &str) -> ImportExportResult<Environment> {
    #[derive(Deserialize)]
    struct PmEnv {
        id: Option<String>,
        name: Option<String>,
        #[serde(default)]
        values: Vec<PmEnvVar>,
    }

    #[derive(Deserialize)]
    struct PmEnvVar {
        #[serde(default)]
        key: String,
        #[serde(default)]
        value: Value,
        #[serde(rename = "type")]
        var_type: Option<String>,
        enabled: Option<bool>,
    }

    let parsed: PmEnv = serde_json::from_str(json_str)?;
    let id = parsed
        .id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4);
    let name = parsed
        .name
        .unwrap_or_else(|| "Imported Environment".to_string());

    let variables = parsed
        .values
        .into_iter()
        .filter(|v| !v.key.is_empty())
        .map(|v| Variable {
            key: v.key,
            value: value_to_string(&v.value),
            enabled: v.enabled.unwrap_or(true),
            secret: v
                .var_type
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("secret")),
        })
        .collect();

    let now = Utc::now();
    Ok(Environment {
        id,
        name,
        variables,
        created_at: now,
        updated_at: now,
    })
}

/// Exports an `Environment` as a Postman Environment JSON string.
pub fn export_postman_environment(env: &Environment) -> ImportExportResult<String> {
    #[derive(Serialize)]
    struct PmEnvOut<'a> {
        id: String,
        name: &'a str,
        values: Vec<PmEnvVarOut<'a>>,
        _postman_variable_scope: &'static str,
        _postman_exported_at: String,
        _postman_exported_using: &'static str,
    }

    #[derive(Serialize)]
    struct PmEnvVarOut<'a> {
        key: &'a str,
        value: &'a str,
        #[serde(rename = "type")]
        var_type: &'static str,
        enabled: bool,
    }

    let out = PmEnvOut {
        id: env.id.to_string(),
        name: &env.name,
        values: env
            .variables
            .iter()
            .map(|v| PmEnvVarOut {
                key: &v.key,
                value: &v.value,
                var_type: if v.secret { "secret" } else { "default" },
                enabled: v.enabled,
            })
            .collect(),
        _postman_variable_scope: "environment",
        _postman_exported_at: Utc::now().to_rfc3339(),
        _postman_exported_using: "apitester",
    };

    Ok(serde_json::to_string_pretty(&out)?)
}

fn parse_item(val: &Value) -> Option<CollectionNode> {
    let obj = val.as_object()?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Untitled")
        .to_string();

    if obj.contains_key("item") && !obj.contains_key("request") {
        let items = obj
            .get("item")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(parse_item).collect())
            .unwrap_or_default();
        let auth = match obj.get("auth") {
            Some(v) if !v.is_null() => parse_auth(v),
            _ => AuthConfig::Inherit,
        };
        let scripts = parse_events(obj.get("event"));
        let description = extract_description(obj.get("description"));

        return Some(CollectionNode::Folder(Folder {
            id: Uuid::new_v4(),
            name,
            description,
            items,
            auth,
            scripts,
        }));
    }

    let req_val = obj.get("request")?;
    let mut req = parse_request_object(req_val, &name);
    if req.description.is_empty() {
        req.description = extract_description(obj.get("description"));
    }
    req.scripts = parse_events(obj.get("event"));
    if let Some(responses) = obj.get("response").and_then(Value::as_array) {
        req.examples = responses.iter().filter_map(parse_saved_example).collect();
    }

    Some(CollectionNode::Request(req))
}

fn parse_request_object(val: &Value, name: &str) -> RequestItem {
    if let Some(url_str) = val.as_str() {
        let (url, params) = split_url_and_query(url_str);
        return RequestItem {
            name: name.to_string(),
            url,
            params,
            ..RequestItem::default()
        };
    }

    let Some(obj) = val.as_object() else {
        return RequestItem {
            name: name.to_string(),
            ..RequestItem::default()
        };
    };

    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .and_then(|m| HttpMethod::from_str(m).ok())
        .unwrap_or(HttpMethod::Get);

    let (url, params) = obj
        .get("url")
        .map(parse_url)
        .unwrap_or_else(|| (String::new(), Vec::new()));

    let headers = parse_headers(obj.get("header"));
    let content_type = headers
        .iter()
        .find(|h| h.enabled && h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.as_str());

    let body = obj
        .get("body")
        .map(|b| parse_body(b, content_type))
        .unwrap_or(RequestBody::None);

    let auth = match obj.get("auth") {
        Some(v) if !v.is_null() => parse_auth(v),
        _ => AuthConfig::Inherit,
    };

    let description = extract_description(obj.get("description"));

    RequestItem {
        id: Uuid::new_v4(),
        name: name.to_string(),
        description,
        method,
        url,
        params,
        headers,
        body,
        auth,
        ..RequestItem::default()
    }
}

fn parse_url(val: &Value) -> (String, Vec<KeyValue>) {
    if let Some(s) = val.as_str() {
        return split_url_and_query(s);
    }
    let Some(obj) = val.as_object() else {
        return (String::new(), Vec::new());
    };

    let query_params: Vec<KeyValue> = obj
        .get("query")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_kv).collect())
        .unwrap_or_default();

    let raw = obj
        .get("raw")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| reconstruct_url_from_parts(obj));

    if !query_params.is_empty() {
        (strip_query(&raw), query_params)
    } else {
        split_url_and_query(&raw)
    }
}

fn reconstruct_url_from_parts(obj: &Map<String, Value>) -> String {
    let mut out = String::new();
    if let Some(proto) = obj.get("protocol").and_then(Value::as_str) {
        if !proto.is_empty() {
            out.push_str(proto);
            out.push_str("://");
        }
    }
    if let Some(host) = obj.get("host") {
        match host {
            Value::String(s) => out.push_str(s),
            Value::Array(parts) => {
                let joined: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                out.push_str(&joined.join("."));
            }
            _ => {}
        }
    }
    if let Some(port) = obj.get("port").and_then(Value::as_str) {
        if !port.is_empty() {
            out.push(':');
            out.push_str(port);
        }
    }
    if let Some(path) = obj.get("path") {
        match path {
            Value::String(s) if !s.is_empty() => {
                if !s.starts_with('/') && !out.is_empty() {
                    out.push('/');
                }
                out.push_str(s);
            }
            Value::Array(parts) if !parts.is_empty() => {
                let joined: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&joined.join("/"));
            }
            _ => {}
        }
    }
    out
}

fn parse_headers(val: Option<&Value>) -> Vec<KeyValue> {
    let Some(val) = val else {
        return Vec::new();
    };
    if let Some(arr) = val.as_array() {
        return arr.iter().filter_map(parse_kv).collect();
    }
    if let Some(raw) = val.as_str() {
        return raw
            .lines()
            .filter_map(|line| {
                let (k, v) = line.split_once(':')?;
                Some(KeyValue::new(k.trim(), v.trim()))
            })
            .collect();
    }
    Vec::new()
}

fn parse_kv(val: &Value) -> Option<KeyValue> {
    let obj = val.as_object()?;
    let key = obj.get("key").map(value_to_string).unwrap_or_default();
    let value = obj.get("value").map(value_to_string).unwrap_or_default();
    let disabled = obj
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let description = extract_description(obj.get("description"));
    Some(KeyValue {
        key,
        value,
        enabled: !disabled,
        description,
    })
}

fn parse_variable(val: &Value) -> Option<Variable> {
    let obj = val.as_object()?;
    let key = obj
        .get("key")
        .or_else(|| obj.get("id"))
        .map(value_to_string)
        .unwrap_or_default();
    if key.is_empty() {
        return None;
    }
    let value = obj.get("value").map(value_to_string).unwrap_or_default();
    let disabled = obj
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let secret = obj
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("secret"));
    Some(Variable {
        key,
        value,
        enabled: !disabled,
        secret,
    })
}

fn parse_body(val: &Value, content_type: Option<&str>) -> RequestBody {
    let Some(obj) = val.as_object() else {
        return RequestBody::None;
    };
    if obj
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return RequestBody::None;
    }

    let mode = obj.get("mode").and_then(Value::as_str).unwrap_or("none");
    match mode {
        "raw" => {
            let content = obj
                .get("raw")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let lang_hint = obj
                .get("options")
                .and_then(|o| o.get("raw"))
                .and_then(|r| r.get("language"))
                .and_then(Value::as_str);
            let language = match lang_hint {
                Some("json") => RawLanguage::Json,
                Some("xml") => RawLanguage::Xml,
                Some("html") => RawLanguage::Html,
                Some("javascript") => RawLanguage::Javascript,
                Some("text") => RawLanguage::Text,
                _ => infer_raw_language(content_type, &content),
            };
            RequestBody::Raw { content, language }
        }
        "urlencoded" => {
            let kvs = obj
                .get("urlencoded")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(parse_kv).collect())
                .unwrap_or_default();
            RequestBody::UrlEncoded(kvs)
        }
        "formdata" => {
            let fields = obj
                .get("formdata")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(parse_form_field).collect())
                .unwrap_or_default();
            RequestBody::FormData(fields)
        }
        "file" => {
            let path = obj
                .get("file")
                .and_then(|f| f.get("src"))
                .map(extract_src)
                .unwrap_or_default();
            RequestBody::Binary { path }
        }
        "graphql" => {
            let gql = obj.get("graphql").and_then(Value::as_object);
            let query = gql
                .and_then(|g| g.get("query"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let variables = gql
                .and_then(|g| g.get("variables"))
                .map(value_to_string)
                .unwrap_or_default();
            RequestBody::GraphQL { query, variables }
        }
        _ => RequestBody::None,
    }
}

fn parse_form_field(val: &Value) -> Option<FormField> {
    let obj = val.as_object()?;
    let key = obj.get("key").map(value_to_string).unwrap_or_default();
    let disabled = obj
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let description = extract_description(obj.get("description"));
    let field_type = obj.get("type").and_then(Value::as_str).unwrap_or("text");

    let kind = if field_type.eq_ignore_ascii_case("file") {
        let path = obj.get("src").map(extract_src).unwrap_or_default();
        let content_type = obj
            .get("contentType")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        FormFieldKind::File { path, content_type }
    } else {
        let value = obj.get("value").map(value_to_string).unwrap_or_default();
        FormFieldKind::Text { value }
    };

    Some(FormField {
        key,
        kind,
        enabled: !disabled,
        description,
    })
}

fn extract_src(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .first()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn parse_events(val: Option<&Value>) -> ScriptConfig {
    let mut scripts = ScriptConfig::default();
    let Some(arr) = val.and_then(Value::as_array) else {
        return scripts;
    };
    for ev in arr {
        let Some(obj) = ev.as_object() else { continue };
        let listen = obj.get("listen").and_then(Value::as_str).unwrap_or("");
        let exec_code = obj
            .get("script")
            .and_then(|s| s.get("exec"))
            .map(|e| match e {
                Value::Array(lines) => lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();

        if exec_code.is_empty() {
            continue;
        }
        match listen {
            "prerequest" => {
                if !scripts.pre_request.is_empty() {
                    scripts.pre_request.push('\n');
                }
                scripts.pre_request.push_str(&exec_code);
            }
            "test" => {
                if !scripts.post_response.is_empty() {
                    scripts.post_response.push('\n');
                }
                scripts.post_response.push_str(&exec_code);
            }
            _ => {}
        }
    }
    scripts
}

fn parse_saved_example(val: &Value) -> Option<SavedExample> {
    let obj = val.as_object()?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Example")
        .to_string();

    let (request_url, request_method, request_headers, request_body) =
        if let Some(orig) = obj.get("originalRequest") {
            let req = parse_request_object(orig, &name);
            let full_url = build_url_with_params(&req.url, &req.params);
            (full_url, req.method, req.headers, req.body)
        } else {
            (String::new(), HttpMethod::Get, Vec::new(), RequestBody::None)
        };

    let status = obj
        .get("code")
        .and_then(Value::as_u64)
        .map(|c| c as u16)
        .unwrap_or(200);
    let status_text = obj
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let resp_headers = parse_headers(obj.get("header"));
    let body_bytes = obj
        .get("body")
        .and_then(Value::as_str)
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_default();
    let size_bytes = body_bytes.len();

    Some(SavedExample {
        id: Uuid::new_v4(),
        name,
        request_url,
        request_method,
        request_headers,
        request_body,
        response: ResponseData {
            status,
            status_text,
            headers: resp_headers,
            body: body_bytes,
            size_bytes,
            ..ResponseData::default()
        },
    })
}

fn auth_kv_map(val: Option<&Value>) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    let Some(val) = val else { return map };
    match val {
        Value::Array(arr) => {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    if let Some(key) = obj.get("key").and_then(Value::as_str) {
                        let v = obj.get("value").cloned().unwrap_or(Value::Null);
                        map.insert(key.to_string(), v);
                    }
                }
            }
        }
        Value::Object(obj) => {
            for (k, v) in obj {
                map.insert(k.clone(), v.clone());
            }
        }
        _ => {}
    }
    map
}

fn map_str(map: &HashMap<String, Value>, key: &str) -> String {
    map.get(key).map(value_to_string).unwrap_or_default()
}

fn map_opt_str(map: &HashMap<String, Value>, key: &str) -> Option<String> {
    let s = map_str(map, key);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn map_bool(map: &HashMap<String, Value>, key: &str, default: bool) -> bool {
    match map.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => default,
        },
        _ => default,
    }
}

fn parse_location(s: &str) -> ApiKeyLocation {
    if s.eq_ignore_ascii_case("query")
        || s.eq_ignore_ascii_case("queryparams")
        || s.eq_ignore_ascii_case("true")
    {
        ApiKeyLocation::Query
    } else {
        ApiKeyLocation::Header
    }
}

fn parse_auth(val: &Value) -> AuthConfig {
    let Some(obj) = val.as_object() else {
        return AuthConfig::NoAuth;
    };
    let auth_type = obj.get("type").and_then(Value::as_str).unwrap_or("noauth");
    let map = auth_kv_map(obj.get(auth_type));

    match auth_type {
        "noauth" => AuthConfig::NoAuth,
        "bearer" => AuthConfig::Bearer {
            token: map_str(&map, "token"),
        },
        "basic" => AuthConfig::Basic {
            username: map_str(&map, "username"),
            password: map_str(&map, "password"),
        },
        "apikey" => AuthConfig::ApiKey {
            key: map_str(&map, "key"),
            value: map_str(&map, "value"),
            add_to: parse_location(&map_str(&map, "in")),
        },
        "oauth2" => {
            let grant_type = match map_str(&map, "grant_type").as_str() {
                "authorization_code_with_pkce" => OAuth2GrantType::AuthorizationCodePkce,
                "client_credentials" => OAuth2GrantType::ClientCredentials,
                "password_credentials" | "password" => OAuth2GrantType::Password,
                "implicit" => OAuth2GrantType::Implicit,
                _ => OAuth2GrantType::AuthorizationCode,
            };
            AuthConfig::OAuth2(OAuth2Config {
                grant_type,
                access_token: map_str(&map, "accessToken"),
                refresh_token: map_opt_str(&map, "refreshToken"),
                token_type: map_str(&map, "tokenType"),
                header_prefix: map_str(&map, "headerPrefix"),
                auth_url: map_str(&map, "authUrl"),
                token_url: map_str(&map, "accessTokenUrl"),
                redirect_uri: map_str(&map, "redirect_uri"),
                client_id: map_str(&map, "clientId"),
                client_secret: map_str(&map, "clientSecret"),
                scope: map_str(&map, "scope"),
                state: map_str(&map, "state"),
                add_token_to: parse_location(&map_str(&map, "addTokenTo")),
                expires_at: None,
            })
        }
        "awsv4" => AuthConfig::AwsV4(AwsV4Config {
            access_key: map_str(&map, "accessKey"),
            secret_key: map_str(&map, "secretKey"),
            region: map_str(&map, "region"),
            service: map_str(&map, "service"),
            session_token: map_opt_str(&map, "sessionToken"),
            add_to: parse_location(&map_str(&map, "addAuthDataToQuery")),
        }),
        "jwt" => {
            let mut cfg = JwtConfig::default();
            let alg = map_str(&map, "algorithm");
            if !alg.is_empty() {
                cfg.algorithm = alg;
            }
            cfg.secret = map_str(&map, "secret");
            cfg.is_secret_base64 = map_bool(&map, "isSecretBase64Encoded", false);
            let payload = map_str(&map, "payload");
            if !payload.is_empty() {
                cfg.payload = payload;
            }
            let prefix = map_str(&map, "headerPrefix");
            if !prefix.is_empty() {
                cfg.header_prefix = prefix;
            }
            let qkey = map_str(&map, "queryParamKey");
            if !qkey.is_empty() {
                cfg.query_param_key = qkey;
            }
            cfg.add_to = parse_location(&map_str(&map, "addTokenTo"));
            cfg.custom_headers = map_opt_str(&map, "header");
            AuthConfig::Jwt(cfg)
        }
        "digest" => AuthConfig::Digest(DigestConfig {
            username: map_str(&map, "username"),
            password: map_str(&map, "password"),
            realm: map_str(&map, "realm"),
            nonce: map_str(&map, "nonce"),
            algorithm: map_str(&map, "algorithm"),
            qop: map_str(&map, "qop"),
            nonce_count: map_str(&map, "nonceCount"),
            client_nonce: map_str(&map, "clientNonce"),
            opaque: map_str(&map, "opaque"),
        }),
        "oauth1" => {
            let defaults = OAuth1Config::default();
            let sig = map_str(&map, "signatureMethod");
            let ver = map_str(&map, "version");
            AuthConfig::OAuth1(OAuth1Config {
                consumer_key: map_str(&map, "consumerKey"),
                consumer_secret: map_str(&map, "consumerSecret"),
                token: map_str(&map, "token"),
                token_secret: map_str(&map, "tokenSecret"),
                signature_method: if sig.is_empty() {
                    defaults.signature_method
                } else {
                    sig
                },
                timestamp: map_opt_str(&map, "timestamp"),
                nonce: map_opt_str(&map, "nonce"),
                version: if ver.is_empty() {
                    defaults.version
                } else {
                    ver
                },
                realm: map_opt_str(&map, "realm"),
                add_params_to_header: map_bool(&map, "addParamsToHeader", true),
                add_empty_params_to_sign: map_bool(&map, "addEmptyParamsToSign", false),
            })
        }
        "ntlm" => AuthConfig::Ntlm(NtlmConfig {
            username: map_str(&map, "username"),
            password: map_str(&map, "password"),
            domain: map_str(&map, "domain"),
            workstation: map_str(&map, "workstation"),
        }),
        "hawk" => {
            let defaults = HawkConfig::default();
            let alg = map_str(&map, "algorithm");
            AuthConfig::Hawk(HawkConfig {
                auth_id: map_str(&map, "authId"),
                auth_key: map_str(&map, "authKey"),
                algorithm: if alg.is_empty() {
                    defaults.algorithm
                } else {
                    alg
                },
                user: map_str(&map, "user"),
                nonce: map_str(&map, "nonce"),
                extra_data: map_str(&map, "extraData"),
                app: map_str(&map, "app"),
                delegation: map_str(&map, "delegation"),
                timestamp: map_opt_str(&map, "timestamp"),
                include_payload_hash: map_bool(&map, "includePayloadHash", false),
            })
        }
        "edgegrid" => AuthConfig::EdgeGrid(EdgeGridConfig {
            access_token: map_str(&map, "accessToken"),
            client_token: map_str(&map, "clientToken"),
            client_secret: map_str(&map, "clientSecret"),
            nonce: map_str(&map, "nonce"),
            timestamp: map_opt_str(&map, "timestamp"),
            base_url: map_str(&map, "baseURL"),
            headers_to_sign: map_str(&map, "headersToSign"),
        }),
        "asap" => {
            let mut cfg = AsapConfig::default();
            let alg = map_str(&map, "alg");
            if !alg.is_empty() {
                cfg.alg = alg;
            }
            cfg.issuer = map_str(&map, "iss");
            cfg.kid = map_str(&map, "kid");
            cfg.audience = map_str(&map, "aud");
            cfg.subject = map_str(&map, "sub");
            cfg.private_key = map_str(&map, "privateKey");
            cfg.claims = map_str(&map, "claims");
            if let Ok(exp) = map_str(&map, "exp").parse::<u64>() {
                cfg.expiry = exp;
            }
            AuthConfig::Asap(cfg)
        }
        _ => AuthConfig::NoAuth,
    }
}

fn export_node(node: &CollectionNode) -> Value {
    match node {
        CollectionNode::Folder(folder) => export_folder(folder),
        CollectionNode::Request(req) => export_request_item(req),
    }
}

fn export_folder(folder: &Folder) -> Value {
    let mut obj = Map::new();
    obj.insert("name".into(), Value::String(folder.name.clone()));
    if !folder.description.is_empty() {
        obj.insert(
            "description".into(),
            Value::String(folder.description.clone()),
        );
    }
    obj.insert(
        "item".into(),
        Value::Array(folder.items.iter().map(export_node).collect()),
    );
    if let Some(auth_val) = export_auth(&folder.auth) {
        obj.insert("auth".into(), auth_val);
    }
    let events = export_events(&folder.scripts);
    if !events.is_empty() {
        obj.insert("event".into(), Value::Array(events));
    }
    Value::Object(obj)
}

fn export_request_item(req: &RequestItem) -> Value {
    let mut item = Map::new();
    item.insert("name".into(), Value::String(req.name.clone()));

    let events = export_events(&req.scripts);
    if !events.is_empty() {
        item.insert("event".into(), Value::Array(events));
    }

    item.insert("request".into(), export_request_body_object(req));

    let responses: Vec<Value> = req.examples.iter().map(export_saved_example).collect();
    item.insert("response".into(), Value::Array(responses));

    Value::Object(item)
}

fn export_request_body_object(req: &RequestItem) -> Value {
    let mut obj = Map::new();
    if let Some(auth_val) = export_auth(&req.auth) {
        obj.insert("auth".into(), auth_val);
    }
    obj.insert("method".into(), Value::String(req.method.as_str().into()));
    obj.insert(
        "header".into(),
        Value::Array(req.headers.iter().map(export_kv).collect()),
    );
    if let Some(body_val) = export_body(&req.body) {
        obj.insert("body".into(), body_val);
    }
    obj.insert("url".into(), export_url(&req.url, &req.params));
    if !req.description.is_empty() {
        obj.insert("description".into(), Value::String(req.description.clone()));
    }
    Value::Object(obj)
}

fn export_url(base_url: &str, params: &[KeyValue]) -> Value {
    let raw = build_url_with_params(base_url, params);
    let mut obj = Map::new();
    obj.insert("raw".into(), Value::String(raw.clone()));

    if let Ok(parsed) = Url::parse(&raw) {
        obj.insert("protocol".into(), Value::String(parsed.scheme().into()));
        if let Some(host) = parsed.host_str() {
            let parts: Vec<Value> = host.split('.').map(|s| Value::String(s.into())).collect();
            obj.insert("host".into(), Value::Array(parts));
        }
        if let Some(port) = parsed.port() {
            obj.insert("port".into(), Value::String(port.to_string()));
        }
        let path_parts: Vec<Value> = parsed
            .path()
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.into()))
            .collect();
        if !path_parts.is_empty() {
            obj.insert("path".into(), Value::Array(path_parts));
        }
    }

    if !params.is_empty() {
        obj.insert(
            "query".into(),
            Value::Array(params.iter().map(export_kv).collect()),
        );
    }
    Value::Object(obj)
}

fn export_kv(kv: &KeyValue) -> Value {
    let mut obj = Map::new();
    obj.insert("key".into(), Value::String(kv.key.clone()));
    obj.insert("value".into(), Value::String(kv.value.clone()));
    if !kv.enabled {
        obj.insert("disabled".into(), Value::Bool(true));
    }
    if !kv.description.is_empty() {
        obj.insert("description".into(), Value::String(kv.description.clone()));
    }
    Value::Object(obj)
}

fn export_variable(var: &Variable) -> Value {
    let mut obj = Map::new();
    obj.insert("key".into(), Value::String(var.key.clone()));
    obj.insert("value".into(), Value::String(var.value.clone()));
    obj.insert(
        "type".into(),
        Value::String(if var.secret { "secret" } else { "string" }.into()),
    );
    if !var.enabled {
        obj.insert("disabled".into(), Value::Bool(true));
    }
    Value::Object(obj)
}

fn export_body(body: &RequestBody) -> Option<Value> {
    match body {
        RequestBody::None => None,
        RequestBody::Raw { content, language } => {
            let lang = match language {
                RawLanguage::Text => "text",
                RawLanguage::Json => "json",
                RawLanguage::Xml => "xml",
                RawLanguage::Html => "html",
                RawLanguage::Javascript => "javascript",
            };
            Some(json!({
                "mode": "raw",
                "raw": content,
                "options": {
                    "raw": { "language": lang }
                }
            }))
        }
        RequestBody::UrlEncoded(kvs) => Some(json!({
            "mode": "urlencoded",
            "urlencoded": kvs.iter().map(export_kv).collect::<Vec<_>>()
        })),
        RequestBody::FormData(fields) => {
            let items: Vec<Value> = fields
                .iter()
                .map(|f| {
                    let mut obj = Map::new();
                    obj.insert("key".into(), Value::String(f.key.clone()));
                    match &f.kind {
                        FormFieldKind::Text { value } => {
                            obj.insert("type".into(), Value::String("text".into()));
                            obj.insert("value".into(), Value::String(value.clone()));
                        }
                        FormFieldKind::File { path, content_type } => {
                            obj.insert("type".into(), Value::String("file".into()));
                            obj.insert("src".into(), Value::String(path.clone()));
                            if let Some(ct) = content_type {
                                obj.insert("contentType".into(), Value::String(ct.clone()));
                            }
                        }
                    }
                    if !f.enabled {
                        obj.insert("disabled".into(), Value::Bool(true));
                    }
                    if !f.description.is_empty() {
                        obj.insert("description".into(), Value::String(f.description.clone()));
                    }
                    Value::Object(obj)
                })
                .collect();
            Some(json!({
                "mode": "formdata",
                "formdata": items
            }))
        }
        RequestBody::Binary { path } => Some(json!({
            "mode": "file",
            "file": { "src": path }
        })),
        RequestBody::GraphQL { query, variables } => Some(json!({
            "mode": "graphql",
            "graphql": {
                "query": query,
                "variables": variables
            }
        })),
    }
}

fn export_events(scripts: &ScriptConfig) -> Vec<Value> {
    let mut events = Vec::new();
    if !scripts.pre_request.is_empty() {
        let lines: Vec<Value> = scripts
            .pre_request
            .lines()
            .map(|l| Value::String(l.into()))
            .collect();
        events.push(json!({
            "listen": "prerequest",
            "script": {
                "type": "text/javascript",
                "exec": lines
            }
        }));
    }
    if !scripts.post_response.is_empty() {
        let lines: Vec<Value> = scripts
            .post_response
            .lines()
            .map(|l| Value::String(l.into()))
            .collect();
        events.push(json!({
            "listen": "test",
            "script": {
                "type": "text/javascript",
                "exec": lines
            }
        }));
    }
    events
}

fn export_saved_example(ex: &SavedExample) -> Value {
    let mut orig_req = Map::new();
    orig_req.insert(
        "method".into(),
        Value::String(ex.request_method.as_str().into()),
    );
    orig_req.insert(
        "header".into(),
        Value::Array(ex.request_headers.iter().map(export_kv).collect()),
    );
    if let Some(body) = export_body(&ex.request_body) {
        orig_req.insert("body".into(), body);
    }
    let (base, params) = split_url_and_query(&ex.request_url);
    orig_req.insert("url".into(), export_url(&base, &params));

    json!({
        "name": ex.name,
        "originalRequest": Value::Object(orig_req),
        "status": ex.response.status_text,
        "code": ex.response.status,
        "header": ex.response.headers.iter().map(export_kv).collect::<Vec<_>>(),
        "body": ex.response.body_text().into_owned(),
    })
}

fn kv_str(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value, "type": "string" })
}

fn kv_bool(key: &str, value: bool) -> Value {
    json!({ "key": key, "value": value, "type": "boolean" })
}

fn loc_str(loc: ApiKeyLocation) -> &'static str {
    match loc {
        ApiKeyLocation::Header => "header",
        ApiKeyLocation::Query => "query",
    }
}

fn export_auth(auth: &AuthConfig) -> Option<Value> {
    match auth {
        AuthConfig::Inherit => None,
        AuthConfig::NoAuth => Some(json!({ "type": "noauth" })),
        AuthConfig::Bearer { token } => Some(json!({
            "type": "bearer",
            "bearer": [kv_str("token", token)]
        })),
        AuthConfig::Basic { username, password } => Some(json!({
            "type": "basic",
            "basic": [kv_str("username", username), kv_str("password", password)]
        })),
        AuthConfig::ApiKey { key, value, add_to } => Some(json!({
            "type": "apikey",
            "apikey": [
                kv_str("key", key),
                kv_str("value", value),
                kv_str("in", loc_str(*add_to))
            ]
        })),
        AuthConfig::OAuth2(c) => {
            let grant = match c.grant_type {
                OAuth2GrantType::AuthorizationCode => "authorization_code",
                OAuth2GrantType::AuthorizationCodePkce => "authorization_code_with_pkce",
                OAuth2GrantType::ClientCredentials => "client_credentials",
                OAuth2GrantType::Password => "password_credentials",
                OAuth2GrantType::Implicit => "implicit",
            };
            let mut entries = vec![
                kv_str("grant_type", grant),
                kv_str("accessToken", &c.access_token),
                kv_str("tokenType", &c.token_type),
                kv_str("headerPrefix", &c.header_prefix),
                kv_str("authUrl", &c.auth_url),
                kv_str("accessTokenUrl", &c.token_url),
                kv_str("redirect_uri", &c.redirect_uri),
                kv_str("clientId", &c.client_id),
                kv_str("clientSecret", &c.client_secret),
                kv_str("scope", &c.scope),
                kv_str("state", &c.state),
                kv_str("addTokenTo", loc_str(c.add_token_to)),
            ];
            if let Some(rt) = &c.refresh_token {
                entries.push(kv_str("refreshToken", rt));
            }
            Some(json!({ "type": "oauth2", "oauth2": entries }))
        }
        AuthConfig::AwsV4(c) => {
            let mut entries = vec![
                kv_str("accessKey", &c.access_key),
                kv_str("secretKey", &c.secret_key),
                kv_str("region", &c.region),
                kv_str("service", &c.service),
                kv_bool("addAuthDataToQuery", c.add_to == ApiKeyLocation::Query),
            ];
            if let Some(st) = &c.session_token {
                entries.push(kv_str("sessionToken", st));
            }
            Some(json!({ "type": "awsv4", "awsv4": entries }))
        }
        AuthConfig::Jwt(c) => {
            let mut entries = vec![
                kv_str("algorithm", &c.algorithm),
                kv_str("secret", &c.secret),
                kv_bool("isSecretBase64Encoded", c.is_secret_base64),
                kv_str("payload", &c.payload),
                kv_str("headerPrefix", &c.header_prefix),
                kv_str("queryParamKey", &c.query_param_key),
                kv_str("addTokenTo", loc_str(c.add_to)),
            ];
            if let Some(ch) = &c.custom_headers {
                entries.push(kv_str("header", ch));
            }
            Some(json!({ "type": "jwt", "jwt": entries }))
        }
        AuthConfig::Digest(c) => Some(json!({
            "type": "digest",
            "digest": [
                kv_str("username", &c.username),
                kv_str("password", &c.password),
                kv_str("realm", &c.realm),
                kv_str("nonce", &c.nonce),
                kv_str("algorithm", &c.algorithm),
                kv_str("qop", &c.qop),
                kv_str("nonceCount", &c.nonce_count),
                kv_str("clientNonce", &c.client_nonce),
                kv_str("opaque", &c.opaque),
            ]
        })),
        AuthConfig::OAuth1(c) => {
            let mut entries = vec![
                kv_str("consumerKey", &c.consumer_key),
                kv_str("consumerSecret", &c.consumer_secret),
                kv_str("token", &c.token),
                kv_str("tokenSecret", &c.token_secret),
                kv_str("signatureMethod", &c.signature_method),
                kv_str("version", &c.version),
                kv_bool("addParamsToHeader", c.add_params_to_header),
                kv_bool("addEmptyParamsToSign", c.add_empty_params_to_sign),
            ];
            if let Some(ts) = &c.timestamp {
                entries.push(kv_str("timestamp", ts));
            }
            if let Some(n) = &c.nonce {
                entries.push(kv_str("nonce", n));
            }
            if let Some(r) = &c.realm {
                entries.push(kv_str("realm", r));
            }
            Some(json!({ "type": "oauth1", "oauth1": entries }))
        }
        AuthConfig::Ntlm(c) => Some(json!({
            "type": "ntlm",
            "ntlm": [
                kv_str("username", &c.username),
                kv_str("password", &c.password),
                kv_str("domain", &c.domain),
                kv_str("workstation", &c.workstation),
            ]
        })),
        AuthConfig::Hawk(c) => {
            let mut entries = vec![
                kv_str("authId", &c.auth_id),
                kv_str("authKey", &c.auth_key),
                kv_str("algorithm", &c.algorithm),
                kv_str("user", &c.user),
                kv_str("nonce", &c.nonce),
                kv_str("extraData", &c.extra_data),
                kv_str("app", &c.app),
                kv_str("delegation", &c.delegation),
                kv_bool("includePayloadHash", c.include_payload_hash),
            ];
            if let Some(ts) = &c.timestamp {
                entries.push(kv_str("timestamp", ts));
            }
            Some(json!({ "type": "hawk", "hawk": entries }))
        }
        AuthConfig::EdgeGrid(c) => {
            let mut entries = vec![
                kv_str("accessToken", &c.access_token),
                kv_str("clientToken", &c.client_token),
                kv_str("clientSecret", &c.client_secret),
                kv_str("nonce", &c.nonce),
                kv_str("baseURL", &c.base_url),
                kv_str("headersToSign", &c.headers_to_sign),
            ];
            if let Some(ts) = &c.timestamp {
                entries.push(kv_str("timestamp", ts));
            }
            Some(json!({ "type": "edgegrid", "edgegrid": entries }))
        }
        AuthConfig::Asap(c) => Some(json!({
            "type": "asap",
            "asap": [
                kv_str("alg", &c.alg),
                kv_str("iss", &c.issuer),
                kv_str("kid", &c.kid),
                kv_str("aud", &c.audience),
                kv_str("sub", &c.subject),
                kv_str("privateKey", &c.private_key),
                kv_str("claims", &c.claims),
                kv_str("exp", &c.expiry.to_string()),
            ]
        })),
    }
}

fn extract_description(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(obj)) => obj
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

fn value_to_string(val: &Value) -> String {
    match val {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}
