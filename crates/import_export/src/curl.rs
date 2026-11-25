use std::str::FromStr;

use apitester_core::models::{
    AuthConfig, FormField, FormFieldKind, HttpMethod, KeyValue, RequestBody, RequestItem,
};
use apitester_core::variables::VariableContext;

use crate::error::{ImportExportError, ImportExportResult};
use crate::util::{
    graphql_json_payload, has_header, infer_raw_language, split_url_and_query, url_path_for_name,
    ResolvedRequest,
};

/// Parses a cURL command (including multiline `\` continuations and shell quoting) into a `RequestItem`.
pub fn import_curl(cmd: &str) -> ImportExportResult<RequestItem> {
    let tokens = tokenize_shell(cmd)?;
    if tokens.is_empty() {
        return Err(ImportExportError::CurlParse("empty cURL command".into()));
    }

    let mut idx = 0;
    if tokens[0].eq_ignore_ascii_case("curl") {
        idx = 1;
    }

    let mut method: Option<HttpMethod> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data_parts: Vec<(String, bool)> = Vec::new();
    let mut urlencoded_parts: Vec<KeyValue> = Vec::new();
    let mut form_fields: Vec<FormField> = Vec::new();
    let mut auth = AuthConfig::Inherit;
    let mut force_get = false;

    while idx < tokens.len() {
        let tok = &tokens[idx];

        if let Some((flag, inline_val)) = split_short_attached_flag(tok) {
            match flag {
                "-X" => {
                    method = Some(HttpMethod::from_str(inline_val).unwrap_or(HttpMethod::Get));
                }
                "-H" => push_header(&mut headers, inline_val),
                "-d" => data_parts.push((inline_val.to_string(), true)),
                "-F" => push_form_field(&mut form_fields, inline_val),
                "-u" => auth = parse_basic_auth(inline_val),
                "-A" => headers.push(KeyValue::new("User-Agent", inline_val)),
                "-b" => headers.push(KeyValue::new("Cookie", inline_val)),
                "-e" => headers.push(KeyValue::new("Referer", inline_val)),
                _ => {}
            }
            idx += 1;
            continue;
        }

        match tok.as_str() {
            "-X" | "--request" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                method = Some(HttpMethod::from_str(val).unwrap_or(HttpMethod::Get));
            }
            "-H" | "--header" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                push_header(&mut headers, val);
            }
            "-d" | "--data" | "--data-binary" | "--data-ascii" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                data_parts.push((val.to_string(), true));
            }
            "--data-raw" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                data_parts.push((val.to_string(), false));
            }
            "--data-urlencode" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                let (k, v) = match val.split_once('=') {
                    Some((k, v)) => (k, v),
                    None => (val, ""),
                };
                urlencoded_parts.push(KeyValue::new(k, v));
            }
            "-F" | "--form" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                push_form_field(&mut form_fields, val);
            }
            "--form-string" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                let (k, v) = val.split_once('=').unwrap_or((val, ""));
                form_fields.push(FormField::text(k, v));
            }
            "-u" | "--user" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                auth = parse_basic_auth(val);
            }
            "-A" | "--user-agent" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                headers.push(KeyValue::new("User-Agent", val));
            }
            "-b" | "--cookie" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                headers.push(KeyValue::new("Cookie", val));
            }
            "-e" | "--referer" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                headers.push(KeyValue::new("Referer", val));
            }
            "--url" => {
                let val = next_arg(&tokens, &mut idx, tok)?;
                url = Some(val.to_string());
            }
            "-G" | "--get" => {
                force_get = true;
            }
            "-I" | "--head" => {
                method = Some(HttpMethod::Head);
            }
            // Boolean flags accepted without error
            "-k" | "--insecure" | "-L" | "--location" | "-s" | "--silent" | "-S"
            | "--show-error" | "-i" | "--include" | "-v" | "--verbose" | "--compressed"
            | "--http1.1" | "--http2" | "-f" | "--fail" | "-N" | "--no-buffer" => {}
            // Flags that take a value we ignore
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-x" | "--proxy"
            | "--cacert" | "--cert" | "--key" | "-c" | "--cookie-jar" | "--retry" => {
                let _ = next_arg(&tokens, &mut idx, tok)?;
            }
            arg if arg.starts_with("--") && arg.contains('=') => {
                if let Some((flag, val)) = arg.split_once('=') {
                    match flag {
                        "--request" => {
                            method = Some(HttpMethod::from_str(val).unwrap_or(HttpMethod::Get));
                        }
                        "--header" => push_header(&mut headers, val),
                        "--data" | "--data-binary" | "--data-ascii" => {
                            data_parts.push((val.to_string(), true));
                        }
                        "--data-raw" => {
                            data_parts.push((val.to_string(), false));
                        }
                        "--data-urlencode" => {
                            let (k, v) = val.split_once('=').unwrap_or((val, ""));
                            urlencoded_parts.push(KeyValue::new(k, v));
                        }
                        "--form" => push_form_field(&mut form_fields, val),
                        "--form-string" => {
                            let (k, v) = val.split_once('=').unwrap_or((val, ""));
                            form_fields.push(FormField::text(k, v));
                        }
                        "--user" => auth = parse_basic_auth(val),
                        "--user-agent" => headers.push(KeyValue::new("User-Agent", val)),
                        "--cookie" => headers.push(KeyValue::new("Cookie", val)),
                        "--url" => url = Some(val.to_string()),
                        _ => {}
                    }
                }
            }
            arg if arg.starts_with('-') && arg.len() > 1 => {
                // Combined short boolean flags such as `-sSLk`
                if arg[1..].chars().all(|c| matches!(c, 'k' | 'L' | 's' | 'S' | 'i' | 'v' | 'f' | 'G' | 'I')) {
                    if arg.contains('G') {
                        force_get = true;
                    }
                    if arg.contains('I') {
                        method = Some(HttpMethod::Head);
                    }
                }
            }
            positional => {
                if url.is_none() {
                    url = Some(positional.to_string());
                }
            }
        }
        idx += 1;
    }

    let raw_url = url.ok_or_else(|| ImportExportError::CurlParse("missing URL in cURL command".into()))?;
    let (clean_url, mut params) = split_url_and_query(&raw_url);

    let has_body_data =
        !data_parts.is_empty() || !urlencoded_parts.is_empty() || !form_fields.is_empty();

    let final_method = if force_get {
        HttpMethod::Get
    } else if let Some(m) = method {
        m
    } else if has_body_data {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    };

    let content_type = headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.as_str());

    let body = if force_get {
        for (part, _) in data_parts {
            for pair in part.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    params.push(KeyValue::new(k, v));
                } else if !pair.is_empty() {
                    params.push(KeyValue::new(pair, ""));
                }
            }
        }
        params.extend(urlencoded_parts);
        RequestBody::None
    } else if !form_fields.is_empty() {
        RequestBody::FormData(form_fields)
    } else if !urlencoded_parts.is_empty() && data_parts.is_empty() {
        RequestBody::UrlEncoded(urlencoded_parts)
    } else if !data_parts.is_empty() {
        let allow_at_file = data_parts.len() == 1 && data_parts[0].1;
        let joined = data_parts
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join("&");
        let is_form_ct = content_type
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("application/x-www-form-urlencoded"));
        if is_form_ct {
            let mut kvs: Vec<KeyValue> = joined
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|pair| {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    KeyValue::new(k, v)
                })
                .collect();
            kvs.extend(urlencoded_parts);
            RequestBody::UrlEncoded(kvs)
        } else if allow_at_file && joined.starts_with('@') && joined.len() > 1 {
            RequestBody::Binary {
                path: joined[1..].to_string(),
            }
        } else {
            let language = infer_raw_language(content_type, &joined);
            RequestBody::Raw {
                content: joined,
                language,
            }
        }
    } else {
        RequestBody::None
    };

    let name = format!("{} {}", final_method.as_str(), url_path_for_name(&clean_url));

    Ok(RequestItem {
        name,
        method: final_method,
        url: clean_url,
        params,
        headers,
        body,
        auth,
        ..RequestItem::default()
    })
}

/// Exports a `RequestItem` as a cURL command string, resolving variables from `vars`.
pub fn export_curl(req: &RequestItem, vars: &VariableContext) -> String {
    let resolved = ResolvedRequest::from_request(req, vars);
    let method_arg = if !resolved.method.is_empty()
        && resolved
            .method
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-')
    {
        resolved.method.clone()
    } else {
        shell_quote(&resolved.method)
    };
    let mut parts: Vec<String> = vec![format!(
        "curl -X {} {}",
        method_arg,
        shell_quote(&resolved.url)
    )];

    if let Some((user, pass)) = &resolved.basic_auth {
        parts.push(format!("-u {}", shell_quote(&format!("{user}:{pass}"))));
    }

    for (k, v) in &resolved.headers {
        parts.push(format!("-H {}", shell_quote(&format!("{k}: {v}"))));
    }

    match &resolved.body {
        RequestBody::None => {}
        RequestBody::Raw { content, .. } => {
            parts.push(format!("--data-raw {}", shell_quote(content)));
        }
        RequestBody::UrlEncoded(kvs) => {
            for kv in kvs {
                parts.push(format!(
                    "--data-urlencode {}",
                    shell_quote(&format!("{}={}", kv.key, kv.value))
                ));
            }
        }
        RequestBody::FormData(fields) => {
            for field in fields {
                match &field.kind {
                    FormFieldKind::Text { value } => {
                        let arg = format!("{}={}", field.key, value);
                        parts.push(format!("--form-string {}", shell_quote(&arg)));
                    }
                    FormFieldKind::File { path, content_type } => {
                        let arg = match content_type {
                            Some(ct) if !ct.is_empty() => format!("{}=@{};type={}", field.key, path, ct),
                            _ => format!("{}=@{}", field.key, path),
                        };
                        parts.push(format!("-F {}", shell_quote(&arg)));
                    }
                }
            }
        }
        RequestBody::Binary { path } => {
            parts.push(format!("--data-binary {}", shell_quote(&format!("@{path}"))));
        }
        RequestBody::GraphQL { query, variables } => {
            if !has_header(&resolved.headers, "content-type") {
                parts.push(format!("-H {}", shell_quote("Content-Type: application/json")));
            }
            let payload = graphql_json_payload(query, variables);
            parts.push(format!("--data-raw {}", shell_quote(&payload)));
        }
    }

    parts.join(" \\\n  ")
}

fn next_arg<'a>(
    tokens: &'a [String],
    idx: &mut usize,
    flag: &str,
) -> ImportExportResult<&'a str> {
    *idx += 1;
    tokens
        .get(*idx)
        .map(String::as_str)
        .ok_or_else(|| ImportExportError::CurlParse(format!("missing argument for {flag}")))
}

fn split_short_attached_flag(tok: &str) -> Option<(&str, &str)> {
    if !tok.starts_with('-') || tok.starts_with("--") || tok.len() <= 2 {
        return None;
    }
    let flag = tok.get(..2)?;
    let rest = tok.get(2..)?;
    if matches!(flag, "-X" | "-H" | "-d" | "-F" | "-u" | "-A" | "-b" | "-e") {
        Some((flag, rest))
    } else {
        None
    }
}

fn push_header(headers: &mut Vec<KeyValue>, raw: &str) {
    if let Some((k, v)) = raw.split_once(':') {
        headers.push(KeyValue::new(k.trim(), v.trim()));
    }
}

fn parse_basic_auth(raw: &str) -> AuthConfig {
    let (user, pass) = raw.split_once(':').unwrap_or((raw, ""));
    AuthConfig::Basic {
        username: user.to_string(),
        password: pass.to_string(),
    }
}

fn push_form_field(fields: &mut Vec<FormField>, raw: &str) {
    let Some((key, val)) = raw.split_once('=') else {
        return;
    };
    if let Some(rest) = val.strip_prefix('@') {
        let (path, content_type) = match rest.split_once(";type=") {
            Some((p, ct)) => (p.trim(), Some(ct.trim().to_string())),
            None => (rest.trim(), None),
        };
        fields.push(FormField {
            key: key.trim().to_string(),
            kind: FormFieldKind::File {
                path: path.to_string(),
                content_type,
            },
            enabled: true,
            description: String::new(),
        });
    } else {
        fields.push(FormField::text(key.trim(), val));
    }
}

/// Shell-quotes a string using single quotes, escaping embedded single quotes as `'\''`.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Tokenizes a POSIX shell command string supporting single quotes, double quotes,
/// `$'...'` ANSI-C strings, backslash escapes, and `\` line continuations.
fn tokenize_shell(input: &str) -> ImportExportResult<Vec<String>> {
    // First strip backslash-newline continuations outside quotes cleanly during lexing.
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            // Line continuation outside quotes
            '\\' if i + 1 < chars.len() && (chars[i + 1] == '\n' || chars[i + 1] == '\r') => {
                i += 1;
                if chars[i] == '\r' && i + 1 < chars.len() && chars[i + 1] == '\n' {
                    i += 1;
                }
                i += 1;
            }
            '\\' if i + 1 < chars.len() => {
                in_token = true;
                current.push(chars[i + 1]);
                i += 2;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '\'' => {
                in_token = true;
                i += 2;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        match chars[i + 1] {
                            'n' => current.push('\n'),
                            'r' => current.push('\r'),
                            't' => current.push('\t'),
                            '\\' => current.push('\\'),
                            '\'' => current.push('\''),
                            '"' => current.push('"'),
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        }
                        i += 2;
                    } else {
                        current.push(chars[i]);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(ImportExportError::CurlParse("unterminated $'...' string".into()));
                }
            }
            '\'' => {
                in_token = true;
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    current.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(ImportExportError::CurlParse("unterminated single quote".into()));
                }
            }
            '"' => {
                in_token = true;
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        match chars[i + 1] {
                            '"' | '\\' | '$' | '`' => {
                                current.push(chars[i + 1]);
                                i += 2;
                            }
                            '\n' => {
                                i += 2;
                            }
                            '\r' if i + 2 < chars.len() && chars[i + 2] == '\n' => {
                                i += 3;
                            }
                            other => {
                                current.push('\\');
                                current.push(other);
                                i += 2;
                            }
                        }
                    } else {
                        current.push(chars[i]);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(ImportExportError::CurlParse("unterminated double quote".into()));
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
                i += 1;
            }
            c => {
                in_token = true;
                current.push(c);
                i += 1;
            }
        }
    }

    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}
