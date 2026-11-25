use apitester_core::models::{FormFieldKind, RequestBody, RequestItem};
use apitester_core::variables::VariableContext;

use crate::curl::export_curl;
use crate::util::{
    base64_encode, encode_query_component, graphql_json_payload, has_header, ResolvedRequest,
};

/// Target language/library for generated HTTP client snippets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodegenLanguage {
    Curl,
    RustReqwest,
    PythonRequests,
    JavaScriptFetch,
    GoNetHttp,
    PhpCurl,
}

/// Generates a ready-to-run HTTP client code snippet for `req` in the requested language.
pub fn generate_code(req: &RequestItem, lang: CodegenLanguage, vars: &VariableContext) -> String {
    match lang {
        CodegenLanguage::Curl => export_curl(req, vars),
        CodegenLanguage::RustReqwest => gen_rust_reqwest(&ResolvedRequest::from_request(req, vars)),
        CodegenLanguage::PythonRequests => {
            gen_python_requests(&ResolvedRequest::from_request(req, vars))
        }
        CodegenLanguage::JavaScriptFetch => gen_js_fetch(&ResolvedRequest::from_request(req, vars)),
        CodegenLanguage::GoNetHttp => gen_go_net_http(&ResolvedRequest::from_request(req, vars)),
        CodegenLanguage::PhpCurl => gen_php_curl(&ResolvedRequest::from_request(req, vars)),
    }
}

fn gen_rust_reqwest(r: &ResolvedRequest) -> String {
    let mut out = String::new();
    out.push_str("use reqwest::Client;\n\n");
    out.push_str("#[tokio::main]\n");
    out.push_str("async fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    out.push_str("    let client = Client::new();\n");

    let method_call = match r.method.as_str() {
        "GET" => format!("client.get({})", rust_str(&r.url)),
        "POST" => format!("client.post({})", rust_str(&r.url)),
        "PUT" => format!("client.put({})", rust_str(&r.url)),
        "PATCH" => format!("client.patch({})", rust_str(&r.url)),
        "DELETE" => format!("client.delete({})", rust_str(&r.url)),
        "HEAD" => format!("client.head({})", rust_str(&r.url)),
        other => format!(
            "client.request(reqwest::Method::from_bytes(b{})?, {})",
            rust_str(other),
            rust_str(&r.url)
        ),
    };

    out.push_str(&format!("    let resp = {method_call}\n"));

    if let Some((user, pass)) = &r.basic_auth {
        out.push_str(&format!(
            "        .basic_auth({}, Some({}))\n",
            rust_str(user),
            rust_str(pass)
        ));
    }

    for (k, v) in &r.headers {
        out.push_str(&format!(
            "        .header({}, {})\n",
            rust_str(k),
            rust_str(v)
        ));
    }

    match &r.body {
        RequestBody::None => {}
        RequestBody::Raw { content, .. } => {
            out.push_str(&format!("        .body({})\n", rust_str(content)));
        }
        RequestBody::UrlEncoded(kvs) => {
            let pairs: Vec<String> = kvs
                .iter()
                .map(|kv| format!("({}, {})", rust_str(&kv.key), rust_str(&kv.value)))
                .collect();
            out.push_str(&format!("        .form(&[{}])\n", pairs.join(", ")));
        }
        RequestBody::FormData(fields) => {
            out.push_str("        .multipart(reqwest::multipart::Form::new()\n");
            for f in fields {
                match &f.kind {
                    FormFieldKind::Text { value } => {
                        out.push_str(&format!(
                            "            .text({}, {})\n",
                            rust_str(&f.key),
                            rust_str(value)
                        ));
                    }
                    FormFieldKind::File { path, .. } => {
                        out.push_str(&format!(
                            "            .file({}, {}).await?\n",
                            rust_str(&f.key),
                            rust_str(path)
                        ));
                    }
                }
            }
            out.push_str("        )\n");
        }
        RequestBody::Binary { path } => {
            out.push_str(&format!(
                "        .body(std::fs::read({})?)\n",
                rust_str(path)
            ));
        }
        RequestBody::GraphQL { query, variables } => {
            if !has_header(&r.headers, "content-type") {
                out.push_str("        .header(\"Content-Type\", \"application/json\")\n");
            }
            let payload = graphql_json_payload(query, variables);
            out.push_str(&format!("        .body({})\n", rust_str(&payload)));
        }
    }

    out.push_str("        .send()\n");
    out.push_str("        .await?;\n\n");
    out.push_str("    println!(\"{}\", resp.text().await?);\n");
    out.push_str("    Ok(())\n");
    out.push_str("}\n");
    out
}

fn gen_python_requests(r: &ResolvedRequest) -> String {
    let mut out = String::from("import requests\n\n");
    out.push_str(&format!("url = {}\n", py_str(&r.url)));

    let mut headers = r.headers.clone();
    if matches!(r.body, RequestBody::GraphQL { .. }) && !has_header(&headers, "content-type") {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }

    if !headers.is_empty() {
        out.push_str("headers = {\n");
        for (k, v) in &headers {
            out.push_str(&format!("    {}: {},\n", py_str(k), py_str(v)));
        }
        out.push_str("}\n");
    }

    let mut args = vec![py_str(&r.method), "url".to_string()];
    if !headers.is_empty() {
        args.push("headers=headers".to_string());
    }
    if let Some((user, pass)) = &r.basic_auth {
        args.push(format!("auth=({}, {})", py_str(user), py_str(pass)));
    }

    match &r.body {
        RequestBody::None => {}
        RequestBody::Raw { content, .. } => {
            out.push_str(&format!("payload = {}\n", py_str(content)));
            args.push("data=payload".to_string());
        }
        RequestBody::UrlEncoded(kvs) => {
            out.push_str("payload = {\n");
            for kv in kvs {
                out.push_str(&format!("    {}: {},\n", py_str(&kv.key), py_str(&kv.value)));
            }
            out.push_str("}\n");
            args.push("data=payload".to_string());
        }
        RequestBody::FormData(fields) => {
            let mut text_fields = Vec::new();
            let mut file_fields = Vec::new();
            for f in fields {
                match &f.kind {
                    FormFieldKind::Text { value } => text_fields.push((&f.key, value)),
                    FormFieldKind::File { path, .. } => file_fields.push((&f.key, path)),
                }
            }
            if !text_fields.is_empty() {
                out.push_str("payload = {\n");
                for (k, v) in text_fields {
                    out.push_str(&format!("    {}: {},\n", py_str(k), py_str(v)));
                }
                out.push_str("}\n");
                args.push("data=payload".to_string());
            }
            if !file_fields.is_empty() {
                out.push_str("files = [\n");
                for (k, p) in file_fields {
                    out.push_str(&format!(
                        "    ({}, open({}, 'rb')),\n",
                        py_str(k),
                        py_str(p)
                    ));
                }
                out.push_str("]\n");
                args.push("files=files".to_string());
            }
        }
        RequestBody::Binary { path } => {
            out.push_str(&format!("with open({}, 'rb') as f:\n    payload = f.read()\n", py_str(path)));
            args.push("data=payload".to_string());
        }
        RequestBody::GraphQL { query, variables } => {
            let payload = graphql_json_payload(query, variables);
            out.push_str(&format!("payload = {}\n", py_str(&payload)));
            args.push("data=payload".to_string());
        }
    }

    out.push_str(&format!(
        "\nresponse = requests.request({})\nprint(response.text)\n",
        args.join(", ")
    ));
    out
}

fn gen_js_fetch(r: &ResolvedRequest) -> String {
    let mut out = String::new();
    let mut headers = r.headers.clone();

    if let Some((user, pass)) = &r.basic_auth {
        if !has_header(&headers, "authorization") {
            let b64 = base64_encode(format!("{user}:{pass}").as_bytes());
            headers.push(("Authorization".to_string(), format!("Basic {b64}")));
        }
    }
    if matches!(r.body, RequestBody::GraphQL { .. }) && !has_header(&headers, "content-type") {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }

    let body_expr: Option<String> = match &r.body {
        RequestBody::None => None,
        RequestBody::Raw { content, .. } => Some(js_str(content)),
        RequestBody::UrlEncoded(kvs) => {
            out.push_str("const params = new URLSearchParams();\n");
            for kv in kvs {
                out.push_str(&format!(
                    "params.append({}, {});\n",
                    js_str(&kv.key),
                    js_str(&kv.value)
                ));
            }
            out.push('\n');
            Some("params".to_string())
        }
        RequestBody::FormData(fields) => {
            out.push_str("const formData = new FormData();\n");
            for f in fields {
                match &f.kind {
                    FormFieldKind::Text { value } => {
                        out.push_str(&format!(
                            "formData.append({}, {});\n",
                            js_str(&f.key),
                            js_str(value)
                        ));
                    }
                    FormFieldKind::File { path, .. } => {
                        out.push_str(&format!(
                            "formData.append({}, fileInput, {});\n",
                            js_str(&f.key),
                            js_str(path)
                        ));
                    }
                }
            }
            out.push('\n');
            Some("formData".to_string())
        }
        RequestBody::Binary { path } => {
            let safe_path = path.replace("*/", "* /").replace(['\r', '\n'], " ");
            Some(format!("/* binary file: {} */ undefined", safe_path))
        }
        RequestBody::GraphQL { query, variables } => {
            let payload = graphql_json_payload(query, variables);
            Some(js_str(&payload))
        }
    };

    out.push_str(&format!("const response = await fetch({}, {{\n", js_str(&r.url)));
    out.push_str(&format!("  method: {},\n", js_str(&r.method)));
    if !headers.is_empty() {
        out.push_str("  headers: {\n");
        for (k, v) in &headers {
            out.push_str(&format!("    {}: {},\n", js_str(k), js_str(v)));
        }
        out.push_str("  },\n");
    }
    if let Some(expr) = body_expr {
        out.push_str(&format!("  body: {expr},\n"));
    }
    out.push_str("});\n\n");
    out.push_str("const text = await response.text();\n");
    out.push_str("console.log(text);\n");
    out
}

fn gen_go_net_http(r: &ResolvedRequest) -> String {
    let mut imports = vec!["\"fmt\"", "\"io\"", "\"net/http\""];
    let mut body_setup = String::new();
    let mut body_arg = "nil".to_string();
    let mut extra_headers = r.headers.clone();

    match &r.body {
        RequestBody::None => {}
        RequestBody::Raw { content, .. } => {
            imports.push("\"strings\"");
            body_setup.push_str(&format!("\tpayload := strings.NewReader({})\n", go_str(content)));
            body_arg = "payload".to_string();
        }
        RequestBody::UrlEncoded(kvs) => {
            imports.push("\"net/url\"");
            imports.push("\"strings\"");
            body_setup.push_str("\tform := url.Values{}\n");
            for kv in kvs {
                body_setup.push_str(&format!(
                    "\tform.Add({}, {})\n",
                    go_str(&kv.key),
                    go_str(&kv.value)
                ));
            }
            body_setup.push_str("\tpayload := strings.NewReader(form.Encode())\n");
            body_arg = "payload".to_string();
            if !has_header(&extra_headers, "content-type") {
                extra_headers.push((
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                ));
            }
        }
        RequestBody::FormData(fields) => {
            imports.push("\"bytes\"");
            imports.push("\"mime/multipart\"");
            body_setup.push_str("\tvar buf bytes.Buffer\n");
            body_setup.push_str("\twriter := multipart.NewWriter(&buf)\n");
            for f in fields {
                match &f.kind {
                    FormFieldKind::Text { value } => {
                        body_setup.push_str(&format!(
                            "\t_ = writer.WriteField({}, {})\n",
                            go_str(&f.key),
                            go_str(value)
                        ));
                    }
                    FormFieldKind::File { path, .. } => {
                        body_setup.push_str(&format!(
                            "\t// attach file {} for field {}\n",
                            go_str(path),
                            go_str(&f.key)
                        ));
                    }
                }
            }
            body_setup.push_str("\t_ = writer.Close()\n");
            body_arg = "&buf".to_string();
        }
        RequestBody::Binary { path } => {
            imports.push("\"os\"");
            body_setup.push_str(&format!(
                "\tpayload, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer payload.Close()\n",
                go_str(path)
            ));
            body_arg = "payload".to_string();
        }
        RequestBody::GraphQL { query, variables } => {
            imports.push("\"strings\"");
            let payload = graphql_json_payload(query, variables);
            body_setup.push_str(&format!("\tpayload := strings.NewReader({})\n", go_str(&payload)));
            body_arg = "payload".to_string();
            if !has_header(&extra_headers, "content-type") {
                extra_headers.push(("Content-Type".to_string(), "application/json".to_string()));
            }
        }
    }

    imports.sort_unstable();
    imports.dedup();

    let mut out = String::from("package main\n\nimport (\n");
    for imp in imports {
        out.push_str(&format!("\t{imp}\n"));
    }
    out.push_str(")\n\nfunc main() {\n");
    out.push_str(&body_setup);
    out.push_str(&format!(
        "\treq, err := http.NewRequest({}, {}, {body_arg})\n",
        go_str(&r.method),
        go_str(&r.url)
    ));
    out.push_str("\tif err != nil {\n\t\tpanic(err)\n\t}\n");

    if let Some((user, pass)) = &r.basic_auth {
        out.push_str(&format!(
            "\treq.SetBasicAuth({}, {})\n",
            go_str(user),
            go_str(pass)
        ));
    }

    for (k, v) in &extra_headers {
        out.push_str(&format!(
            "\treq.Header.Set({}, {})\n",
            go_str(k),
            go_str(v)
        ));
    }

    if matches!(r.body, RequestBody::FormData(_)) {
        out.push_str("\treq.Header.Set(\"Content-Type\", writer.FormDataContentType())\n");
    }

    out.push_str("\n\tresp, err := http.DefaultClient.Do(req)\n");
    out.push_str("\tif err != nil {\n\t\tpanic(err)\n\t}\n");
    out.push_str("\tdefer resp.Body.Close()\n\n");
    out.push_str("\tbody, _ := io.ReadAll(resp.Body)\n");
    out.push_str("\tfmt.Println(string(body))\n");
    out.push_str("}\n");
    out
}

fn gen_php_curl(r: &ResolvedRequest) -> String {
    let mut out = String::from("<?php\n\n$ch = curl_init();\n\n");
    out.push_str("curl_setopt_array($ch, [\n");
    out.push_str(&format!("    CURLOPT_URL => {},\n", php_str(&r.url)));
    out.push_str("    CURLOPT_RETURNTRANSFER => true,\n");
    out.push_str(&format!(
        "    CURLOPT_CUSTOMREQUEST => {},\n",
        php_str(&r.method)
    ));

    if let Some((user, pass)) = &r.basic_auth {
        out.push_str(&format!(
            "    CURLOPT_USERPWD => {},\n",
            php_str(&format!("{user}:{pass}"))
        ));
    }

    let mut headers = r.headers.clone();
    if matches!(r.body, RequestBody::GraphQL { .. }) && !has_header(&headers, "content-type") {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }

    if !headers.is_empty() {
        out.push_str("    CURLOPT_HTTPHEADER => [\n");
        for (k, v) in &headers {
            out.push_str(&format!("        {},\n", php_str(&format!("{k}: {v}"))));
        }
        out.push_str("    ],\n");
    }

    match &r.body {
        RequestBody::None => {}
        RequestBody::Raw { content, .. } => {
            out.push_str(&format!("    CURLOPT_POSTFIELDS => {},\n", php_str(content)));
        }
        RequestBody::UrlEncoded(kvs) => {
            let encoded = kvs
                .iter()
                .map(|kv| {
                    format!(
                        "{}={}",
                        encode_query_component(&kv.key),
                        encode_query_component(&kv.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("&");
            out.push_str(&format!("    CURLOPT_POSTFIELDS => {},\n", php_str(&encoded)));
        }
        RequestBody::FormData(fields) => {
            out.push_str("    CURLOPT_POSTFIELDS => [\n");
            for f in fields {
                match &f.kind {
                    FormFieldKind::Text { value } => {
                        out.push_str(&format!(
                            "        {} => {},\n",
                            php_str(&f.key),
                            php_str(value)
                        ));
                    }
                    FormFieldKind::File { path, .. } => {
                        out.push_str(&format!(
                            "        {} => new CURLFile({}),\n",
                            php_str(&f.key),
                            php_str(path)
                        ));
                    }
                }
            }
            out.push_str("    ],\n");
        }
        RequestBody::Binary { path } => {
            out.push_str(&format!(
                "    CURLOPT_POSTFIELDS => file_get_contents({}),\n",
                php_str(path)
            ));
        }
        RequestBody::GraphQL { query, variables } => {
            let payload = graphql_json_payload(query, variables);
            out.push_str(&format!("    CURLOPT_POSTFIELDS => {},\n", php_str(&payload)));
        }
    }

    out.push_str("]);\n\n");
    out.push_str("$response = curl_exec($ch);\n");
    out.push_str("curl_close($ch);\n");
    out.push_str("echo $response;\n");
    out
}

fn rust_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn go_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn py_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

fn php_str(s: &str) -> String {
    format!(
        "'{}'",
        s.replace('\\', "\\\\").replace('\'', "\\'")
    )
}
