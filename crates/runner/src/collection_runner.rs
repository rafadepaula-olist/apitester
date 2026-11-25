use std::collections::HashMap;
use std::time::{Duration, Instant};

use apitester_auth::{
    apply_auth, resolve_auth, AsapConfig, AuthConfig, AuthRequestContext, AwsV4Config,
    DigestConfig, EdgeGridConfig, HawkConfig, JwtConfig, NtlmConfig, OAuth1Config, OAuth2Config,
};
use apitester_core::models::{
    AppSettings, Collection, CollectionNode, ConsoleLogEntry, RequestBody, RequestItem,
    ResponseData, TestResult,
};
use apitester_core::variables::{VariableContext, VariableResolver, VariableScope};
use apitester_net::{HttpClient, HttpRequestOptions};
use apitester_scripting::{run_script, ScriptExecutionInput};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::RunnerError;

/// Configuration for running a [`Collection`].
#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub iterations: usize,
    pub delay_ms: u64,
    pub data_rows: Vec<HashMap<String, String>>,
    pub stop_on_error: bool,
    pub script_timeout_ms: u64,
    pub settings: AppSettings,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            iterations: 1,
            delay_ms: 0,
            data_rows: Vec::new(),
            stop_on_error: false,
            script_timeout_ms: 5_000,
            settings: AppSettings::default(),
        }
    }
}

/// Live progress events emitted by [`run_collection`] when a channel is provided.
#[derive(Debug, Clone)]
pub enum RunnerProgressEvent {
    IterationStarted {
        iteration: usize,
        total_iterations: usize,
    },
    RequestCompleted {
        iteration: usize,
        result: Box<RequestRunResult>,
    },
    IterationFinished {
        iteration: usize,
    },
    RunFinished,
}

/// Outcome of executing a single request during a collection run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRunResult {
    pub iteration: usize,
    pub request_id: Uuid,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: f64,
    pub response: Option<ResponseData>,
    pub test_results: Vec<TestResult>,
    pub console_logs: Vec<ConsoleLogEntry>,
    pub error: Option<String>,
}

/// Aggregated report returned after [`run_collection`] completes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollectionRunReport {
    pub collection_name: String,
    pub iterations: usize,
    pub results: Vec<RequestRunResult>,
    pub total_requests: usize,
    pub failed_requests: usize,
    pub passed_tests: usize,
    pub failed_tests: usize,
    pub total_duration_ms: f64,
    #[serde(skip_serializing, default)]
    pub updated_globals: HashMap<String, String>,
    #[serde(skip_serializing, default)]
    pub updated_collection_vars: HashMap<String, String>,
    #[serde(skip_serializing, default)]
    pub updated_environment_vars: HashMap<String, String>,
}

/// Parses a CSV data file (header row + data rows) into iteration data rows.
pub fn parse_csv_data(csv_str: &str) -> Result<Vec<HashMap<String, String>>, RunnerError> {
    let content = csv_str.strip_prefix('\u{feff}').unwrap_or(csv_str);
    let rows = parse_csv_rows(content)?;
    let mut iter = rows.into_iter().filter(|r| !r.iter().all(|c| c.trim().is_empty()));

    let Some(headers) = iter.next() else {
        return Ok(Vec::new());
    };
    let headers: Vec<String> = headers.into_iter().map(|h| h.trim().to_string()).collect();
    if headers.iter().all(|h| h.is_empty()) {
        return Err(RunnerError::InvalidCsv("CSV header row is empty".into()));
    }

    let mut out = Vec::new();
    for row in iter {
        let mut map = HashMap::with_capacity(headers.len());
        for (idx, header) in headers.iter().enumerate() {
            if header.is_empty() {
                continue;
            }
            let val = row.get(idx).cloned().unwrap_or_default();
            map.insert(header.clone(), val);
        }
        out.push(map);
    }

    Ok(out)
}

/// Parses a JSON array of objects into iteration data rows.
pub fn parse_json_data(json_str: &str) -> Result<Vec<HashMap<String, String>>, RunnerError> {
    let raw_rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_str(json_str)?;
    let mut out = Vec::with_capacity(raw_rows.len());

    for obj in raw_rows {
        let mut row = HashMap::with_capacity(obj.len());
        for (k, v) in obj {
            let s = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            row.insert(k, s);
        }
        out.push(row);
    }

    Ok(out)
}

struct PlannedRequest<'a> {
    request: &'a RequestItem,
    /// Ordered from innermost (request) to outermost (collection) for `resolve_auth`.
    auth_chain: Vec<&'a AuthConfig>,
    /// Ordered from outermost (collection) to innermost (request).
    pre_scripts: Vec<&'a str>,
    /// Ordered from outermost (collection) to innermost (request).
    post_scripts: Vec<&'a str>,
}

/// Executes all requests in `collection` across configured iterations with variable scoping,
/// auth inheritance, pre-request & post-response scripts, and `setNextRequest` flow control.
pub async fn run_collection(
    collection: &Collection,
    initial_vars: VariableContext,
    options: RunnerOptions,
    progress_tx: Option<mpsc::UnboundedSender<RunnerProgressEvent>>,
) -> CollectionRunReport {
    let run_started = Instant::now();
    let mut vars = VariableContext::new();
    vars.load_variables(VariableScope::Collection, &collection.variables);
    // Overlay initial_vars so caller-provided values take precedence.
    vars.global.extend(initial_vars.global);
    vars.collection.extend(initial_vars.collection);
    vars.environment.extend(initial_vars.environment);
    vars.data.extend(initial_vars.data);
    vars.local.extend(initial_vars.local);

    let planned = flatten_collection(collection);
    let total_iterations = if options.iterations == 0 && !options.data_rows.is_empty() {
        options.data_rows.len()
    } else {
        options.iterations.max(1)
    };

    let client = HttpClient::new();
    let mut report = CollectionRunReport {
        collection_name: collection.name.clone(),
        iterations: total_iterations,
        ..Default::default()
    };

    let mut aborted = false;
    for iteration in 0..total_iterations {
        if aborted {
            break;
        }

        vars.local.clear();
        vars.data = options
            .data_rows
            .get(iteration)
            .cloned()
            .unwrap_or_default();

        if let Some(tx) = &progress_tx {
            let _ = tx.send(RunnerProgressEvent::IterationStarted {
                iteration,
                total_iterations,
            });
        }

        let max_steps = planned.len().saturating_mul(10).saturating_add(100);
        let mut idx = 0usize;
        let mut steps = 0usize;

        while idx < planned.len() && steps < max_steps {
            steps += 1;
            let plan = &planned[idx];
            let req = plan.request;
            let req_started = Instant::now();

            let mut all_tests = Vec::new();
            let mut all_logs = Vec::new();
            let mut next_request: Option<String> = None;
            let mut stop_next_request = false;
            let mut error_msg: Option<String> = None;

            // 1. Run pre-request scripts (collection -> folders -> request).
            for script in &plan.pre_scripts {
                if script.trim().is_empty() {
                    continue;
                }
                let script_owned = script.to_string();
                let script_input = build_script_input(req, None, &vars);
                let timeout_ms = options.script_timeout_ms;
                let out = tokio::task::spawn_blocking(move || {
                    run_script(&script_owned, script_input, timeout_ms)
                })
                .await
                .unwrap_or_default();
                if out.error.is_none() {
                    apply_script_vars(&mut vars, &out);
                }
                all_tests.extend(out.test_results);
                all_logs.extend(out.console_logs);
                if out.stop_next_request {
                    stop_next_request = true;
                    next_request = None;
                } else if let Some(nr) = out.next_request {
                    stop_next_request = false;
                    next_request = Some(nr);
                }
                if let Some(err) = out.error {
                    error_msg = Some(format!("pre-request script error: {err}"));
                    break;
                }
            }

            let resolver = VariableResolver::new(vars.clone());
            let resolved_url = resolver.resolve(&req.url);
            let mut response_data: Option<ResponseData> = None;

            // 2. Resolve auth & execute HTTP request if pre-request succeeded.
            if error_msg.is_none() {
                let effective_auth = resolve_auth(&plan.auth_chain);
                let resolved_auth = resolve_auth_variables(effective_auth, &resolver);
                let auth_ctx = build_auth_context(req, &resolver);

                match apply_auth(&resolved_auth, &auth_ctx) {
                    Ok(auth_app) => {
                        let http_opts = HttpRequestOptions {
                            extra_headers: auth_app.headers,
                            extra_query: auth_app.query_params,
                            cookies: Vec::new(),
                            settings: options.settings.clone(),
                        };

                        match client.execute(req, &vars, &http_opts).await {
                            Ok(mut resp) => {
                                all_logs.extend(resp.console_logs.clone());

                                // 3. Run post-response test scripts.
                                for script in &plan.post_scripts {
                                    if script.trim().is_empty() {
                                        continue;
                                    }
                                    let script_owned = script.to_string();
                                    let script_input =
                                        build_script_input(req, Some(resp.clone()), &vars);
                                    let timeout_ms = options.script_timeout_ms;
                                    let out = tokio::task::spawn_blocking(move || {
                                        run_script(&script_owned, script_input, timeout_ms)
                                    })
                                    .await
                                    .unwrap_or_default();
                                    if out.error.is_none() {
                                        apply_script_vars(&mut vars, &out);
                                    }
                                    all_tests.extend(out.test_results);
                                    all_logs.extend(out.console_logs);
                                    if out.stop_next_request {
                                        stop_next_request = true;
                                        next_request = None;
                                    } else if let Some(nr) = out.next_request {
                                        stop_next_request = false;
                                        next_request = Some(nr);
                                    }
                                    if let Some(err) = out.error {
                                        error_msg =
                                            Some(format!("post-response script error: {err}"));
                                        break;
                                    }
                                }

                                resp.test_results = all_tests.clone();
                                resp.console_logs = all_logs.clone();
                                response_data = Some(resp);
                            }
                            Err(err) => {
                                error_msg = Some(err.to_string());
                            }
                        }
                    }
                    Err(err) => {
                        error_msg = Some(format!("auth error: {err}"));
                    }
                }
            }

            let duration_ms = response_data
                .as_ref()
                .map(|r| r.timings.total_ms)
                .unwrap_or_else(|| req_started.elapsed().as_secs_f64() * 1000.0);

            let passed_in_req = all_tests.iter().filter(|t| t.passed).count();
            let failed_in_req = all_tests.len() - passed_in_req;
            report.total_requests += 1;
            report.passed_tests += passed_in_req;
            report.failed_tests += failed_in_req;
            if error_msg.is_some() {
                report.failed_requests += 1;
            }

            let run_result = RequestRunResult {
                iteration,
                request_id: req.id,
                request_name: req.name.clone(),
                method: req.method.as_str().to_string(),
                url: resolved_url,
                status: response_data.as_ref().map(|r| r.status),
                duration_ms,
                response: response_data,
                test_results: all_tests,
                console_logs: all_logs,
                error: error_msg.clone(),
            };

            report.results.push(run_result.clone());
            if let Some(tx) = &progress_tx {
                let _ = tx.send(RunnerProgressEvent::RequestCompleted {
                    iteration,
                    result: Box::new(run_result),
                });
            }

            if error_msg.is_some() && options.stop_on_error {
                aborted = true;
                break;
            }

            // 4. Advance index using `setNextRequest` or sequential order.
            if stop_next_request {
                break;
            } else if let Some(target_name) = next_request {
                if let Some(pos) = planned.iter().position(|p| {
                    p.request.name == target_name || p.request.id.to_string() == target_name
                }) {
                    idx = pos;
                } else {
                    break;
                }
            } else {
                idx += 1;
            }

            if options.delay_ms > 0 && idx < planned.len() {
                tokio::time::sleep(Duration::from_millis(options.delay_ms)).await;
            }
        }

        if let Some(tx) = &progress_tx {
            let _ = tx.send(RunnerProgressEvent::IterationFinished { iteration });
        }
    }

    report.total_duration_ms = run_started.elapsed().as_secs_f64() * 1000.0;
    report.updated_globals = vars.global;
    report.updated_collection_vars = vars.collection;
    report.updated_environment_vars = vars.environment;

    if let Some(tx) = &progress_tx {
        let _ = tx.send(RunnerProgressEvent::RunFinished);
    }

    report
}

fn flatten_collection<'a>(collection: &'a Collection) -> Vec<PlannedRequest<'a>> {
    let mut out = Vec::new();
    let mut auth_stack = vec![&collection.auth];
    let mut pre_stack = vec![collection.scripts.pre_request.as_str()];
    let mut post_stack = vec![collection.scripts.post_response.as_str()];

    collect_nodes(
        &collection.items,
        &mut auth_stack,
        &mut pre_stack,
        &mut post_stack,
        &mut out,
    );
    out
}

fn collect_nodes<'a>(
    nodes: &'a [CollectionNode],
    auth_stack: &mut Vec<&'a AuthConfig>,
    pre_stack: &mut Vec<&'a str>,
    post_stack: &mut Vec<&'a str>,
    out: &mut Vec<PlannedRequest<'a>>,
) {
    for node in nodes {
        match node {
            CollectionNode::Folder(folder) => {
                auth_stack.push(&folder.auth);
                pre_stack.push(folder.scripts.pre_request.as_str());
                post_stack.push(folder.scripts.post_response.as_str());
                collect_nodes(&folder.items, auth_stack, pre_stack, post_stack, out);
                auth_stack.pop();
                pre_stack.pop();
                post_stack.pop();
            }
            CollectionNode::Request(req) => {
                let mut auth_chain = Vec::with_capacity(auth_stack.len() + 1);
                auth_chain.push(&req.auth);
                for parent_auth in auth_stack.iter().rev() {
                    auth_chain.push(*parent_auth);
                }

                let mut pre_scripts = pre_stack.clone();
                pre_scripts.push(req.scripts.pre_request.as_str());

                let mut post_scripts = post_stack.clone();
                post_scripts.push(req.scripts.post_response.as_str());

                out.push(PlannedRequest {
                    request: req,
                    auth_chain,
                    pre_scripts,
                    post_scripts,
                });
            }
        }
    }
}

fn build_script_input(
    req: &RequestItem,
    response: Option<ResponseData>,
    vars: &VariableContext,
) -> ScriptExecutionInput {
    ScriptExecutionInput {
        request: req.clone(),
        response,
        globals: vars.global.clone(),
        collection_vars: vars.collection.clone(),
        environment_vars: vars.environment.clone(),
        local_vars: vars.local.clone(),
        data_vars: vars.data.clone(),
    }
}

fn apply_script_vars(
    vars: &mut VariableContext,
    out: &apitester_scripting::ScriptExecutionOutput,
) {
    vars.global = out.updated_globals.clone();
    vars.collection = out.updated_collection_vars.clone();
    vars.environment = out.updated_environment_vars.clone();
    vars.local = out.updated_local_vars.clone();
}

pub(crate) fn build_auth_context(
    req: &RequestItem,
    resolver: &VariableResolver,
) -> AuthRequestContext {
    let raw_url = resolver.resolve(&req.url);
    let url = if let Ok(mut parsed) = url::Url::parse(&raw_url) {
        {
            let mut pairs = parsed.query_pairs_mut();
            for kv in &req.params {
                if kv.enabled && !kv.key.is_empty() {
                    pairs.append_pair(&resolver.resolve(&kv.key), &resolver.resolve(&kv.value));
                }
            }
        }
        if parsed.query() == Some("") {
            parsed.set_query(None);
        }
        parsed.to_string()
    } else {
        raw_url
    };

    let headers = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.is_empty())
        .map(|h| (resolver.resolve(&h.key), resolver.resolve(&h.value)))
        .collect();

    let body = match &req.body {
        RequestBody::Raw { content, .. } => resolver.resolve(content).into_bytes(),
        RequestBody::UrlEncoded(fields) => {
            let pairs: Vec<(String, String)> = fields
                .iter()
                .filter(|kv| kv.enabled && !kv.key.is_empty())
                .map(|kv| (resolver.resolve(&kv.key), resolver.resolve(&kv.value)))
                .collect();
            serde_urlencoded::to_string(&pairs)
                .unwrap_or_default()
                .into_bytes()
        }
        RequestBody::GraphQL { query, variables } => {
            let resolved_query = resolver.resolve(query);
            let resolved_vars = resolver.resolve(variables);
            let vars_json: serde_json::Value = if resolved_vars.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&resolved_vars).unwrap_or_else(|_| serde_json::json!({}))
            };
            serde_json::json!({
                "query": resolved_query,
                "variables": vars_json,
            })
            .to_string()
            .into_bytes()
        }
        RequestBody::Binary { path } => {
            const MAX_AUTH_BODY_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB
            let resolved_path = resolver.resolve(path);
            match std::fs::metadata(&resolved_path) {
                Ok(meta) if meta.is_file() && meta.len() <= MAX_AUTH_BODY_BYTES => {
                    std::fs::read(&resolved_path).unwrap_or_default()
                }
                _ => Vec::new(),
            }
        }
        RequestBody::None | RequestBody::FormData(_) => Vec::new(),
    };

    AuthRequestContext {
        method: req.method.as_str().to_string(),
        url,
        headers,
        body,
    }
}

fn resolve_opt(opt: &Option<String>, resolver: &VariableResolver) -> Option<String> {
    opt.as_deref().map(|s| resolver.resolve(s))
}

pub(crate) fn resolve_auth_variables(
    auth: &AuthConfig,
    resolver: &VariableResolver,
) -> AuthConfig {
    match auth {
        AuthConfig::Inherit => AuthConfig::Inherit,
        AuthConfig::NoAuth => AuthConfig::NoAuth,
        AuthConfig::Bearer { token } => AuthConfig::Bearer {
            token: resolver.resolve(token),
        },
        AuthConfig::Basic { username, password } => AuthConfig::Basic {
            username: resolver.resolve(username),
            password: resolver.resolve(password),
        },
        AuthConfig::ApiKey { key, value, add_to } => AuthConfig::ApiKey {
            key: resolver.resolve(key),
            value: resolver.resolve(value),
            add_to: *add_to,
        },
        AuthConfig::OAuth2(c) => AuthConfig::OAuth2(OAuth2Config {
            grant_type: c.grant_type,
            access_token: resolver.resolve(&c.access_token),
            refresh_token: resolve_opt(&c.refresh_token, resolver),
            token_type: resolver.resolve(&c.token_type),
            header_prefix: resolver.resolve(&c.header_prefix),
            auth_url: resolver.resolve(&c.auth_url),
            token_url: resolver.resolve(&c.token_url),
            redirect_uri: resolver.resolve(&c.redirect_uri),
            client_id: resolver.resolve(&c.client_id),
            client_secret: resolver.resolve(&c.client_secret),
            scope: resolver.resolve(&c.scope),
            state: resolver.resolve(&c.state),
            add_token_to: c.add_token_to,
            expires_at: c.expires_at,
        }),
        AuthConfig::AwsV4(c) => AuthConfig::AwsV4(AwsV4Config {
            access_key: resolver.resolve(&c.access_key),
            secret_key: resolver.resolve(&c.secret_key),
            region: resolver.resolve(&c.region),
            service: resolver.resolve(&c.service),
            session_token: resolve_opt(&c.session_token, resolver),
            add_to: c.add_to,
        }),
        AuthConfig::Jwt(c) => AuthConfig::Jwt(JwtConfig {
            algorithm: resolver.resolve(&c.algorithm),
            secret: resolver.resolve(&c.secret),
            is_secret_base64: c.is_secret_base64,
            payload: resolver.resolve(&c.payload),
            header_prefix: resolver.resolve(&c.header_prefix),
            query_param_key: resolver.resolve(&c.query_param_key),
            add_to: c.add_to,
            custom_headers: resolve_opt(&c.custom_headers, resolver),
        }),
        AuthConfig::Digest(c) => AuthConfig::Digest(DigestConfig {
            username: resolver.resolve(&c.username),
            password: resolver.resolve(&c.password),
            realm: resolver.resolve(&c.realm),
            nonce: resolver.resolve(&c.nonce),
            algorithm: resolver.resolve(&c.algorithm),
            qop: resolver.resolve(&c.qop),
            nonce_count: resolver.resolve(&c.nonce_count),
            client_nonce: resolver.resolve(&c.client_nonce),
            opaque: resolver.resolve(&c.opaque),
        }),
        AuthConfig::OAuth1(c) => AuthConfig::OAuth1(OAuth1Config {
            consumer_key: resolver.resolve(&c.consumer_key),
            consumer_secret: resolver.resolve(&c.consumer_secret),
            token: resolver.resolve(&c.token),
            token_secret: resolver.resolve(&c.token_secret),
            signature_method: resolver.resolve(&c.signature_method),
            timestamp: resolve_opt(&c.timestamp, resolver),
            nonce: resolve_opt(&c.nonce, resolver),
            version: resolver.resolve(&c.version),
            realm: resolve_opt(&c.realm, resolver),
            add_params_to_header: c.add_params_to_header,
            add_empty_params_to_sign: c.add_empty_params_to_sign,
        }),
        AuthConfig::Ntlm(c) => AuthConfig::Ntlm(NtlmConfig {
            username: resolver.resolve(&c.username),
            password: resolver.resolve(&c.password),
            domain: resolver.resolve(&c.domain),
            workstation: resolver.resolve(&c.workstation),
        }),
        AuthConfig::Hawk(c) => AuthConfig::Hawk(HawkConfig {
            auth_id: resolver.resolve(&c.auth_id),
            auth_key: resolver.resolve(&c.auth_key),
            algorithm: resolver.resolve(&c.algorithm),
            user: resolver.resolve(&c.user),
            nonce: resolver.resolve(&c.nonce),
            extra_data: resolver.resolve(&c.extra_data),
            app: resolver.resolve(&c.app),
            delegation: resolver.resolve(&c.delegation),
            timestamp: resolve_opt(&c.timestamp, resolver),
            include_payload_hash: c.include_payload_hash,
        }),
        AuthConfig::EdgeGrid(c) => AuthConfig::EdgeGrid(EdgeGridConfig {
            access_token: resolver.resolve(&c.access_token),
            client_token: resolver.resolve(&c.client_token),
            client_secret: resolver.resolve(&c.client_secret),
            nonce: resolver.resolve(&c.nonce),
            timestamp: resolve_opt(&c.timestamp, resolver),
            base_url: resolver.resolve(&c.base_url),
            headers_to_sign: resolver.resolve(&c.headers_to_sign),
        }),
        AuthConfig::Asap(c) => AuthConfig::Asap(AsapConfig {
            alg: resolver.resolve(&c.alg),
            issuer: resolver.resolve(&c.issuer),
            kid: resolver.resolve(&c.kid),
            audience: resolver.resolve(&c.audience),
            subject: resolver.resolve(&c.subject),
            private_key: resolver.resolve(&c.private_key),
            claims: resolver.resolve(&c.claims),
            expiry: c.expiry,
        }),
    }
}

fn parse_csv_rows(input: &str) -> Result<Vec<Vec<String>>, RunnerError> {
    let mut rows = Vec::new();
    let mut current_row = Vec::new();
    let mut current_field = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        current_field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                _ => current_field.push(ch),
            }
        } else {
            match ch {
                '"' if current_field.is_empty() => in_quotes = true,
                ',' => {
                    current_row.push(std::mem::take(&mut current_field));
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    current_row.push(std::mem::take(&mut current_field));
                    rows.push(std::mem::take(&mut current_row));
                }
                '\n' => {
                    current_row.push(std::mem::take(&mut current_field));
                    rows.push(std::mem::take(&mut current_row));
                }
                _ => current_field.push(ch),
            }
        }
    }

    if in_quotes {
        return Err(RunnerError::InvalidCsv("unterminated quoted field".into()));
    }

    if !current_field.is_empty() || !current_row.is_empty() {
        current_row.push(current_field);
        rows.push(current_row);
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apitester_core::models::{Folder, HttpMethod, ScriptConfig, Variable};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn start_mock_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req_str.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                    let has_bearer = req_str
                        .lines()
                        .any(|l| l.to_ascii_lowercase().starts_with("authorization: bearer secret-tok"));

                    let body = serde_json::json!({
                        "path": path,
                        "authorized": has_bearer
                    })
                    .to_string();

                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        (addr, handle)
    }

    #[test]
    fn parses_csv_and_json_data_files() {
        let csv = "name,role\n\"Ada, Lovelace\",admin\nGrace,\"engineer\"\"lead\"\"\"\n";
        let csv_rows = parse_csv_data(csv).expect("csv");
        assert_eq!(csv_rows.len(), 2);
        assert_eq!(csv_rows[0]["name"], "Ada, Lovelace");
        assert_eq!(csv_rows[0]["role"], "admin");
        assert_eq!(csv_rows[1]["name"], "Grace");
        assert_eq!(csv_rows[1]["role"], "engineer\"lead\"");

        let json = r#"[{"id": 1, "user": "ada", "active": true}, {"id": 2, "user": "grace", "active": null}]"#;
        let json_rows = parse_json_data(json).expect("json");
        assert_eq!(json_rows.len(), 2);
        assert_eq!(json_rows[0]["id"], "1");
        assert_eq!(json_rows[0]["user"], "ada");
        assert_eq!(json_rows[0]["active"], "true");
        assert_eq!(json_rows[1]["active"], "");
    }

    #[tokio::test]
    async fn runs_collection_with_auth_inheritance_data_rows_scripts_and_next_request() {
        let (addr, server_handle) = start_mock_server().await;

        let mut req1 = RequestItem::new("Step 1", HttpMethod::Get, format!("http://{addr}/users/{{{{user}}}}"));
        req1.scripts = ScriptConfig {
            pre_request: r#"pm.environment.set("ran_step_1", "yes");"#.into(),
            post_response: r#"
                pm.test("step 1 authorized", () => {
                    const j = pm.response.json();
                    pm.expect(j.authorized).to.be.true;
                });
                pm.execution.setNextRequest("Step 3");
            "#
            .into(),
        };

        let req2 = RequestItem::new("Step 2", HttpMethod::Get, format!("http://{addr}/skipped"));

        let mut req3 = RequestItem::new("Step 3", HttpMethod::Get, format!("http://{addr}/done/{{{{user}}}}"));
        req3.scripts.post_response = r#"
            pm.test("step 3 status 200", () => {
                pm.response.to.have.status(200);
            });
        "#
        .into();

        let mut folder = Folder::new("Main Folder");
        folder.items.push(CollectionNode::Request(req1));
        folder.items.push(CollectionNode::Request(req2));
        folder.items.push(CollectionNode::Request(req3));

        let mut collection = Collection::new("Demo Collection");
        collection.auth = AuthConfig::Bearer {
            token: "{{api_token}}".into(),
        };
        collection
            .variables
            .push(Variable::new("api_token", "secret-tok"));
        collection.items.push(CollectionNode::Folder(folder));

        let data_rows = parse_csv_data("user\nada\ngrace\n").expect("data");
        let (tx, mut rx) = mpsc::unbounded_channel();

        let report = run_collection(
            &collection,
            VariableContext::new(),
            RunnerOptions {
                iterations: 2,
                data_rows,
                ..Default::default()
            },
            Some(tx),
        )
        .await;

        // 2 iterations x 2 executed requests (Step 2 skipped via setNextRequest).
        assert_eq!(report.total_requests, 4);
        assert_eq!(report.failed_requests, 0);
        assert_eq!(report.passed_tests, 4);
        assert_eq!(report.failed_tests, 0);
        assert_eq!(
            report.updated_environment_vars.get("ran_step_1").map(String::as_str),
            Some("yes")
        );
        assert!(report.results.iter().all(|r| r.request_name != "Step 2"));
        assert!(report.results[0].url.ends_with("/users/ada"));
        assert!(report.results[2].url.ends_with("/users/grace"));

        let mut events = 0;
        while rx.try_recv().is_ok() {
            events += 1;
        }
        assert!(events >= 6);

        server_handle.abort();
    }

    #[test]
    fn build_auth_context_and_resolve_auth_variables_cover_params_bodies_and_variants() {
        let mut vars = VariableContext::new();
        vars.set(VariableScope::Environment, "host", "api.example.com");
        vars.set(VariableScope::Environment, "qval", "hello world");
        vars.set(VariableScope::Environment, "ak", "AKIA_TEST");
        vars.set(VariableScope::Environment, "sk", "SECRET_TEST");
        let resolver = VariableResolver::new(vars);

        let mut req = RequestItem::new("Form", HttpMethod::Post, "https://{{host}}/items");
        req.params
            .push(apitester_core::models::KeyValue::new("search", "{{qval}}"));
        req.body = RequestBody::UrlEncoded(vec![apitester_core::models::KeyValue::new(
            "name",
            "{{qval}}",
        )]);

        let ctx = build_auth_context(&req, &resolver);
        assert_eq!(ctx.url, "https://api.example.com/items?search=hello+world");
        assert_eq!(ctx.body, b"name=hello+world");

        let mut gql = RequestItem::new("GQL", HttpMethod::Post, "https://{{host}}/graphql");
        gql.body = RequestBody::GraphQL {
            query: "{ ping }".into(),
            variables: r#"{"n": 42}"#.into(),
        };
        let gql_ctx = build_auth_context(&gql, &resolver);
        let parsed: serde_json::Value = serde_json::from_slice(&gql_ctx.body).expect("json");
        assert_eq!(parsed["variables"]["n"], 42);

        let aws = AuthConfig::AwsV4(AwsV4Config {
            access_key: "{{ak}}".into(),
            secret_key: "{{sk}}".into(),
            region: "us-east-1".into(),
            service: "s3".into(),
            ..Default::default()
        });
        let resolved = resolve_auth_variables(&aws, &resolver);
        match resolved {
            AuthConfig::AwsV4(c) => {
                assert_eq!(c.access_key, "AKIA_TEST");
                assert_eq!(c.secret_key, "SECRET_TEST");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_next_request_null_stops_iteration() {
        let (addr, server_handle) = start_mock_server().await;

        let mut req1 = RequestItem::new("First", HttpMethod::Get, format!("http://{addr}/first"));
        req1.scripts.post_response = "pm.execution.setNextRequest(null);".into();
        let req2 = RequestItem::new("Second", HttpMethod::Get, format!("http://{addr}/second"));

        let mut col = Collection::new("Stop Test");
        col.items.push(CollectionNode::Request(req1));
        col.items.push(CollectionNode::Request(req2));

        let report = run_collection(&col, VariableContext::new(), RunnerOptions::default(), None).await;
        assert_eq!(report.total_requests, 1);
        assert_eq!(report.results[0].request_name, "First");

        server_handle.abort();
    }
}
