use std::collections::HashMap;
use std::str::FromStr;

use apitester_core::models::{
    Collection, CollectionNode, Folder, FormField, FormFieldKind, HttpMethod, KeyValue,
    RawLanguage, RequestBody, RequestItem, Variable,
};
use chrono::Utc;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::{ImportExportError, ImportExportResult};

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Imports an OpenAPI 3.x or Swagger 2.0 specification (JSON or YAML) into a `Collection`.
pub fn import_openapi(content: &str) -> ImportExportResult<Collection> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(ImportExportError::InvalidFormat(
            "empty OpenAPI document".into(),
        ));
    }

    let doc: Value = if trimmed.starts_with('{') {
        serde_json::from_str(trimmed)?
    } else {
        serde_yaml::from_str(trimmed)?
    };

    let root = doc
        .as_object()
        .ok_or_else(|| ImportExportError::InvalidFormat("expected root mapping".into()))?;

    let is_openapi3 = root
        .get("openapi")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with("3."));
    let is_swagger2 = root
        .get("swagger")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with("2."));

    if !is_openapi3 && !is_swagger2 {
        let ver = root
            .get("openapi")
            .or_else(|| root.get("swagger"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(ImportExportError::UnsupportedVersion(format!(
            "unsupported OpenAPI/Swagger version: {ver}"
        )));
    }

    let info = root.get("info").and_then(Value::as_object);
    let name = info
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Imported API")
        .to_string();
    let description = info
        .and_then(|i| i.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let base_url = if is_openapi3 {
        extract_openapi3_base_url(root)
    } else {
        extract_swagger2_base_url(root)
    };

    let mut variables = Vec::new();
    if !base_url.is_empty() {
        variables.push(Variable::new("baseUrl", &base_url));
    }

    // Preserve declared tag order and descriptions.
    let mut tag_order: Vec<String> = Vec::new();
    let mut tag_descriptions: HashMap<String, String> = HashMap::new();
    if let Some(tags) = root.get("tags").and_then(Value::as_array) {
        for tag in tags {
            if let Some(t_name) = tag.get("name").and_then(Value::as_str) {
                if !tag_order.iter().any(|t| t == t_name) {
                    tag_order.push(t_name.to_string());
                }
                if let Some(desc) = tag.get("description").and_then(Value::as_str) {
                    tag_descriptions.insert(t_name.to_string(), desc.to_string());
                }
            }
        }
    }

    let mut tagged_requests: HashMap<String, Vec<CollectionNode>> = HashMap::new();
    let mut untagged_requests: Vec<CollectionNode> = Vec::new();

    if let Some(paths) = root.get("paths").and_then(Value::as_object) {
        for (path, raw_path_item) in paths {
            let path_item = resolve_ref(&doc, raw_path_item);
            let Some(path_obj) = path_item.as_object() else {
                continue;
            };

            let path_params = path_obj
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            for &method_key in HTTP_METHODS {
                let Some(op_val) = path_obj.get(method_key) else {
                    continue;
                };
                let Some(op) = op_val.as_object() else {
                    continue;
                };

                let method = HttpMethod::from_str(method_key).unwrap_or(HttpMethod::Get);
                let op_name = op
                    .get("summary")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        op.get("operationId")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                    })
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{} {}", method.as_str(), path));

                let op_desc = op
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                let merged_params = merge_parameters(
                    &doc,
                    &path_params,
                    op.get("parameters").and_then(Value::as_array),
                );

                let mut query_params = Vec::new();
                let mut headers = Vec::new();
                let mut swagger_body_param: Option<&Value> = None;
                let mut swagger_form_params: Vec<&Value> = Vec::new();

                for param in &merged_params {
                    let Some(p_obj) = param.as_object() else {
                        continue;
                    };
                    let p_name = p_obj.get("name").and_then(Value::as_str).unwrap_or("");
                    if p_name.is_empty() {
                        continue;
                    }
                    let p_in = p_obj.get("in").and_then(Value::as_str).unwrap_or("");
                    let p_desc = p_obj
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let p_val = extract_param_example(&doc, p_obj);

                    match p_in {
                        "query" => query_params.push(KeyValue {
                            key: p_name.to_string(),
                            value: p_val,
                            enabled: true,
                            description: p_desc,
                        }),
                        "header" => headers.push(KeyValue {
                            key: p_name.to_string(),
                            value: p_val,
                            enabled: true,
                            description: p_desc,
                        }),
                        "body" => swagger_body_param = Some(param),
                        "formData" => swagger_form_params.push(param),
                        _ => {}
                    }
                }

                let body = if is_openapi3 {
                    extract_openapi3_body(&doc, op.get("requestBody"), &mut headers)
                } else {
                    extract_swagger2_body(
                        &doc,
                        swagger_body_param,
                        &swagger_form_params,
                        op.get("consumes").or_else(|| root.get("consumes")),
                        &mut headers,
                    )
                };

                let full_url = join_base_and_path(&base_url, path);

                let req = RequestItem {
                    id: Uuid::new_v4(),
                    name: op_name,
                    description: op_desc,
                    method,
                    url: full_url,
                    params: query_params,
                    headers,
                    body,
                    ..RequestItem::default()
                };

                let first_tag = op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(Value::as_str);

                if let Some(tag) = first_tag {
                    if !tag_order.iter().any(|t| t == tag) {
                        tag_order.push(tag.to_string());
                    }
                    tagged_requests
                        .entry(tag.to_string())
                        .or_default()
                        .push(CollectionNode::Request(req));
                } else {
                    untagged_requests.push(CollectionNode::Request(req));
                }
            }
        }
    }

    let mut items = Vec::new();
    for tag in tag_order {
        if let Some(reqs) = tagged_requests.remove(&tag) {
            let desc = tag_descriptions.remove(&tag).unwrap_or_default();
            let mut folder = Folder::new(tag);
            folder.description = desc;
            folder.items = reqs;
            items.push(CollectionNode::Folder(folder));
        }
    }
    items.extend(untagged_requests);

    let now = Utc::now();
    Ok(Collection {
        id: Uuid::new_v4(),
        name,
        description,
        items,
        variables,
        created_at: now,
        updated_at: now,
        ..Collection::default()
    })
}

fn extract_openapi3_base_url(root: &Map<String, Value>) -> String {
    let Some(server) = root
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(Value::as_object)
    else {
        return String::new();
    };

    let mut url = server
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if let Some(vars) = server.get("variables").and_then(Value::as_object) {
        for (k, v) in vars {
            if let Some(default_val) = v.get("default").and_then(Value::as_str) {
                url = url.replace(&format!("{{{k}}}"), default_val);
            }
        }
    }
    url
}

fn extract_swagger2_base_url(root: &Map<String, Value>) -> String {
    let host = root.get("host").and_then(Value::as_str).unwrap_or("");
    let base_path = root.get("basePath").and_then(Value::as_str).unwrap_or("");
    if host.is_empty() {
        return base_path.to_string();
    }
    let scheme = root
        .get("schemes")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(Value::as_str)
        .unwrap_or("https");
    let clean_base = base_path.trim_end_matches('/');
    format!("{scheme}://{host}{clean_base}")
}

fn join_base_and_path(base: &str, path: &str) -> String {
    if base.is_empty() {
        return path.to_string();
    }
    let b = base.trim_end_matches('/');
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        b.to_string()
    } else {
        format!("{b}/{p}")
    }
}

fn resolve_ref<'a>(doc: &'a Value, val: &'a Value) -> &'a Value {
    let mut current = val;
    for _ in 0..8 {
        let Some(ref_str) = current.get("$ref").and_then(Value::as_str) else {
            return current;
        };
        let Some(pointer) = ref_str.strip_prefix('#') else {
            return current;
        };
        if let Some(target) = doc.pointer(pointer) {
            current = target;
        } else {
            return current;
        }
    }
    current
}

fn merge_parameters(
    doc: &Value,
    path_params: &[Value],
    op_params: Option<&Vec<Value>>,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut index_by_key: HashMap<(String, String), usize> = HashMap::new();

    for raw in path_params
        .iter()
        .chain(op_params.into_iter().flat_map(|v| v.iter()))
    {
        let resolved = resolve_ref(doc, raw).clone();
        let name = resolved
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let loc = resolved
            .get("in")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let key = (name, loc);
        if let Some(&existing_idx) = index_by_key.get(&key) {
            out[existing_idx] = resolved;
        } else {
            index_by_key.insert(key, out.len());
            out.push(resolved);
        }
    }
    out
}

fn extract_param_example(doc: &Value, p_obj: &Map<String, Value>) -> String {
    if let Some(ex) = p_obj.get("example") {
        return scalar_to_string(ex);
    }
    if let Some(def) = p_obj.get("default") {
        return scalar_to_string(def);
    }
    if let Some(schema) = p_obj.get("schema") {
        let resolved = resolve_ref(doc, schema);
        if let Some(ex) = resolved.get("example").or_else(|| resolved.get("default")) {
            return scalar_to_string(ex);
        }
    }
    String::new()
}

fn extract_openapi3_body(
    doc: &Value,
    raw_body: Option<&Value>,
    headers: &mut Vec<KeyValue>,
) -> RequestBody {
    let Some(raw_body) = raw_body else {
        return RequestBody::None;
    };
    let body_val = resolve_ref(doc, raw_body);
    let Some(content) = body_val.get("content").and_then(Value::as_object) else {
        return RequestBody::None;
    };

    // Prefer JSON if present, otherwise first content type.
    let chosen = content
        .iter()
        .find(|(ct, _)| ct.to_ascii_lowercase().contains("json"))
        .or_else(|| content.iter().next());

    let Some((mime, media_obj)) = chosen else {
        return RequestBody::None;
    };

    let lower = mime.to_ascii_lowercase();
    if lower.contains("application/x-www-form-urlencoded") {
        let kvs = schema_to_form_kvs(doc, media_obj.get("schema"));
        return RequestBody::UrlEncoded(kvs);
    }
    if lower.contains("multipart/form-data") {
        let fields = schema_to_multipart_fields(doc, media_obj.get("schema"));
        return RequestBody::FormData(fields);
    }

    if !headers
        .iter()
        .any(|h| h.key.eq_ignore_ascii_case("content-type"))
    {
        headers.push(KeyValue::new("Content-Type", mime.clone()));
    }

    let language = if lower.contains("json") {
        RawLanguage::Json
    } else if lower.contains("xml") {
        RawLanguage::Xml
    } else if lower.contains("html") {
        RawLanguage::Html
    } else {
        RawLanguage::Text
    };

    let example_val = media_obj
        .get("example")
        .cloned()
        .or_else(|| {
            media_obj
                .get("examples")
                .and_then(Value::as_object)
                .and_then(|exs| exs.values().next())
                .and_then(|ex| resolve_ref(doc, ex).get("value").cloned())
        })
        .or_else(|| {
            media_obj
                .get("schema")
                .map(|s| synthesize_schema_example(doc, s, 0))
        });

    let body_str = match example_val {
        Some(Value::String(s)) if language != RawLanguage::Json => s,
        Some(val) => serde_json::to_string_pretty(&val).unwrap_or_default(),
        None => String::new(),
    };

    RequestBody::Raw {
        content: body_str,
        language,
    }
}

fn extract_swagger2_body(
    doc: &Value,
    body_param: Option<&Value>,
    form_params: &[&Value],
    consumes: Option<&Value>,
    headers: &mut Vec<KeyValue>,
) -> RequestBody {
    if let Some(bp) = body_param {
        if !headers
            .iter()
            .any(|h| h.key.eq_ignore_ascii_case("content-type"))
        {
            headers.push(KeyValue::new("Content-Type", "application/json"));
        }
        let sample = bp
            .get("schema")
            .map(|s| synthesize_schema_example(doc, s, 0))
            .unwrap_or_else(|| Value::Object(Map::new()));
        return RequestBody::Raw {
            content: serde_json::to_string_pretty(&sample).unwrap_or_default(),
            language: RawLanguage::Json,
        };
    }

    if !form_params.is_empty() {
        let is_multipart = consumes
            .and_then(Value::as_array)
            .is_some_and(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .any(|c| c.contains("multipart/form-data"))
            })
            || form_params
                .iter()
                .any(|p| p.get("type").and_then(Value::as_str) == Some("file"));

        if is_multipart {
            let fields = form_params
                .iter()
                .filter_map(|p| {
                    let obj = p.as_object()?;
                    let key = obj.get("name")?.as_str()?.to_string();
                    let desc = obj
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let kind = if obj.get("type").and_then(Value::as_str) == Some("file") {
                        FormFieldKind::File {
                            path: String::new(),
                            content_type: None,
                        }
                    } else {
                        FormFieldKind::Text {
                            value: extract_param_example(doc, obj),
                        }
                    };
                    Some(FormField {
                        key,
                        kind,
                        enabled: true,
                        description: desc,
                    })
                })
                .collect();
            return RequestBody::FormData(fields);
        }

        let kvs = form_params
            .iter()
            .filter_map(|p| {
                let obj = p.as_object()?;
                let key = obj.get("name")?.as_str()?.to_string();
                let value = extract_param_example(doc, obj);
                let desc = obj
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(KeyValue {
                    key,
                    value,
                    enabled: true,
                    description: desc,
                })
            })
            .collect();
        return RequestBody::UrlEncoded(kvs);
    }

    RequestBody::None
}

fn schema_to_form_kvs(doc: &Value, schema: Option<&Value>) -> Vec<KeyValue> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let resolved = resolve_ref(doc, schema);
    let Some(props) = resolved.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    props
        .iter()
        .map(|(k, prop)| {
            let p = resolve_ref(doc, prop);
            let val = p
                .get("example")
                .or_else(|| p.get("default"))
                .map(scalar_to_string)
                .unwrap_or_default();
            KeyValue::new(k.clone(), val)
        })
        .collect()
}

fn schema_to_multipart_fields(doc: &Value, schema: Option<&Value>) -> Vec<FormField> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let resolved = resolve_ref(doc, schema);
    let Some(props) = resolved.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    props
        .iter()
        .map(|(k, prop)| {
            let p = resolve_ref(doc, prop);
            let format = p.get("format").and_then(Value::as_str).unwrap_or("");
            if format == "binary" || format == "byte" {
                FormField::file(k.clone(), "")
            } else {
                let val = p
                    .get("example")
                    .or_else(|| p.get("default"))
                    .map(scalar_to_string)
                    .unwrap_or_default();
                FormField::text(k.clone(), val)
            }
        })
        .collect()
}

fn synthesize_schema_example(doc: &Value, schema: &Value, depth: usize) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    let resolved = resolve_ref(doc, schema);
    if let Some(ex) = resolved.get("example") {
        return ex.clone();
    }
    if let Some(def) = resolved.get("default") {
        return def.clone();
    }
    if let Some(enum_vals) = resolved.get("enum").and_then(Value::as_array) {
        if let Some(first) = enum_vals.first() {
            return first.clone();
        }
    }

    let ty = resolved.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "object" => {
            let mut map = Map::new();
            if let Some(props) = resolved.get("properties").and_then(Value::as_object) {
                for (k, prop) in props {
                    map.insert(k.clone(), synthesize_schema_example(doc, prop, depth + 1));
                }
            }
            Value::Object(map)
        }
        "array" => {
            if let Some(items) = resolved.get("items") {
                Value::Array(vec![synthesize_schema_example(doc, items, depth + 1)])
            } else {
                Value::Array(Vec::new())
            }
        }
        "integer" => Value::Number(0.into()),
        "number" => Value::Number(0.into()),
        "boolean" => Value::Bool(false),
        "string" => Value::String(String::new()),
        _ => {
            if resolved.get("properties").is_some() {
                let mut map = Map::new();
                if let Some(props) = resolved.get("properties").and_then(Value::as_object) {
                    for (k, prop) in props {
                        map.insert(k.clone(), synthesize_schema_example(doc, prop, depth + 1));
                    }
                }
                Value::Object(map)
            } else {
                Value::Object(Map::new())
            }
        }
    }
}

fn scalar_to_string(val: &Value) -> String {
    match val {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}
