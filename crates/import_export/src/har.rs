use std::str::FromStr;

use apitester_core::models::{
    Collection, CollectionNode, FormField, FormFieldKind, HttpMethod, KeyValue, RequestBody,
    RequestItem, ResponseData, SavedExample,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{ImportExportError, ImportExportResult};
use crate::util::{infer_raw_language, split_url_and_query, strip_query, url_path_for_name};

#[derive(Deserialize)]
struct HarDocument {
    log: HarLog,
}

#[derive(Deserialize)]
struct HarLog {
    #[serde(default)]
    creator: Option<HarCreator>,
    #[serde(default)]
    entries: Vec<HarEntry>,
}

#[derive(Deserialize)]
struct HarCreator {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct HarEntry {
    request: HarRequest,
    #[serde(default)]
    response: Option<HarResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<HarNameValue>,
    #[serde(default)]
    query_string: Vec<HarNameValue>,
    #[serde(default)]
    post_data: Option<HarPostData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarPostData {
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    params: Vec<HarPostParam>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarPostParam {
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarResponse {
    #[serde(default)]
    status: u16,
    #[serde(default)]
    status_text: String,
    #[serde(default)]
    headers: Vec<HarNameValue>,
    #[serde(default)]
    content: Option<HarContent>,
}

#[derive(Deserialize)]
struct HarContent {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct HarNameValue {
    name: String,
    #[serde(default)]
    value: String,
}

/// Imports a HAR 1.2 JSON capture into a `Collection`.
pub fn import_har(json_str: &str) -> ImportExportResult<Collection> {
    let har: HarDocument = serde_json::from_str(json_str)?;

    let name = har
        .log
        .creator
        .map(|c| c.name)
        .filter(|n| !n.trim().is_empty())
        .map(|n| format!("HAR Import ({n})"))
        .unwrap_or_else(|| "HAR Import".to_string());

    let mut items = Vec::with_capacity(har.log.entries.len());

    for entry in har.log.entries {
        if entry.request.url.trim().is_empty() {
            return Err(ImportExportError::InvalidFormat(
                "HAR entry missing request URL".into(),
            ));
        }

        let method = HttpMethod::from_str(&entry.request.method).unwrap_or(HttpMethod::Get);

        let (url, params) = if !entry.request.query_string.is_empty() {
            let params = entry
                .request
                .query_string
                .into_iter()
                .map(|q| KeyValue::new(q.name, q.value))
                .collect();
            (strip_query(&entry.request.url), params)
        } else {
            split_url_and_query(&entry.request.url)
        };

        // Filter out HTTP/2 pseudo-headers (`:method`, `:path`, etc.)
        let headers: Vec<KeyValue> = entry
            .request
            .headers
            .into_iter()
            .filter(|h| !h.name.starts_with(':'))
            .map(|h| KeyValue::new(h.name, h.value))
            .collect();

        let body = match entry.request.post_data {
            Some(pd) => parse_har_post_data(pd),
            None => RequestBody::None,
        };

        let req_name = format!("{} {}", method.as_str(), url_path_for_name(&url));

        let mut examples = Vec::new();
        if let Some(resp) = entry.response {
            if resp.status > 0 {
                let body_bytes = resp
                    .content
                    .and_then(|c| c.text)
                    .map(|t| t.into_bytes())
                    .unwrap_or_default();
                let size_bytes = body_bytes.len();
                let resp_headers = resp
                    .headers
                    .into_iter()
                    .filter(|h| !h.name.starts_with(':'))
                    .map(|h| KeyValue::new(h.name, h.value))
                    .collect();

                examples.push(SavedExample {
                    id: Uuid::new_v4(),
                    name: format!("{} {}", resp.status, resp.status_text).trim().to_string(),
                    request_url: entry.request.url.clone(),
                    request_method: method.clone(),
                    request_headers: headers.clone(),
                    request_body: body.clone(),
                    response: ResponseData {
                        status: resp.status,
                        status_text: resp.status_text,
                        headers: resp_headers,
                        body: body_bytes,
                        size_bytes,
                        ..ResponseData::default()
                    },
                });
            }
        }

        items.push(CollectionNode::Request(RequestItem {
            id: Uuid::new_v4(),
            name: req_name,
            method,
            url,
            params,
            headers,
            body,
            examples,
            ..RequestItem::default()
        }));
    }

    let now = Utc::now();
    Ok(Collection {
        id: Uuid::new_v4(),
        name,
        items,
        created_at: now,
        updated_at: now,
        ..Collection::default()
    })
}

fn parse_har_post_data(pd: HarPostData) -> RequestBody {
    let lower = pd.mime_type.to_ascii_lowercase();
    if lower.contains("application/x-www-form-urlencoded") && !pd.params.is_empty() {
        let kvs = pd
            .params
            .into_iter()
            .map(|p| KeyValue::new(p.name, p.value.unwrap_or_default()))
            .collect();
        return RequestBody::UrlEncoded(kvs);
    }

    if lower.contains("multipart/form-data") && !pd.params.is_empty() {
        let fields = pd
            .params
            .into_iter()
            .map(|p| {
                if let Some(file_name) = p.file_name {
                    FormField {
                        key: p.name,
                        kind: FormFieldKind::File {
                            path: file_name,
                            content_type: p.content_type,
                        },
                        enabled: true,
                        description: String::new(),
                    }
                } else {
                    FormField::text(p.name, p.value.unwrap_or_default())
                }
            })
            .collect();
        return RequestBody::FormData(fields);
    }

    if !pd.text.is_empty() {
        let language = infer_raw_language(
            if pd.mime_type.is_empty() {
                None
            } else {
                Some(pd.mime_type.as_str())
            },
            &pd.text,
        );
        return RequestBody::Raw {
            content: pd.text,
            language,
        };
    }

    RequestBody::None
}
