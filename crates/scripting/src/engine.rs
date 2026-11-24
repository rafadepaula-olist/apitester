use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apitester_core::models::{
    ConsoleLogEntry, ConsoleLogLevel, KeyValue, RequestBody, RequestItem, ResponseData, TestResult,
};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::Utc;
use rquickjs::{CatchResultExt, CaughtError, Context, Exception, Function, Runtime};
use serde::{Deserialize, Serialize};

use crate::{ScriptExecutionInput, ScriptExecutionOutput};

const PRELUDE_JS: &str = include_str!("prelude.js");
const MAX_MEMORY_BYTES: usize = 32 * 1024 * 1024;
const MAX_STACK_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
struct HeaderPayload<'a> {
    key: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct RequestPayload<'a> {
    url: &'a str,
    method: &'a str,
    headers: Vec<HeaderPayload<'a>>,
    body: Option<&'a str>,
}

#[derive(Serialize)]
struct ResponsePayload<'a> {
    code: u16,
    status: &'a str,
    response_time: f64,
    headers: Vec<HeaderPayload<'a>>,
    body: String,
}

#[derive(Serialize)]
struct InitPayload<'a> {
    request: RequestPayload<'a>,
    response: Option<ResponsePayload<'a>>,
    globals: &'a HashMap<String, String>,
    collection_vars: &'a HashMap<String, String>,
    environment_vars: &'a HashMap<String, String>,
    local_vars: &'a HashMap<String, String>,
    data_vars: &'a HashMap<String, String>,
}

#[derive(Deserialize)]
struct DumpState {
    globals: HashMap<String, String>,
    collection_vars: HashMap<String, String>,
    environment_vars: HashMap<String, String>,
    local_vars: HashMap<String, String>,
    next_request: Option<String>,
    #[serde(default)]
    stop_next_request: bool,
}

fn enabled_headers(headers: &[KeyValue]) -> Vec<HeaderPayload<'_>> {
    headers
        .iter()
        .filter(|h| h.enabled)
        .map(|h| HeaderPayload {
            key: h.key.as_str(),
            value: h.value.as_str(),
        })
        .collect()
}

fn build_request_payload(req: &RequestItem) -> RequestPayload<'_> {
    let body = match &req.body {
        RequestBody::Raw { content, .. } => Some(content.as_str()),
        RequestBody::GraphQL { query, .. } => Some(query.as_str()),
        _ => None,
    };

    RequestPayload {
        url: req.url.as_str(),
        method: req.method.as_str(),
        headers: enabled_headers(&req.headers),
        body,
    }
}

fn build_response_payload(resp: &ResponseData) -> ResponsePayload<'_> {
    ResponsePayload {
        code: resp.status,
        status: resp.status_text.as_str(),
        response_time: resp.timings.total_ms,
        headers: enabled_headers(&resp.headers),
        body: resp.body_text().into_owned(),
    }
}

fn parse_log_level(level: &str) -> ConsoleLogLevel {
    match level {
        "info" => ConsoleLogLevel::Info,
        "warn" => ConsoleLogLevel::Warn,
        "error" => ConsoleLogLevel::Error,
        _ => ConsoleLogLevel::Log,
    }
}

fn encode_btoa(input: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(input.len());
    for ch in input.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return Err("InvalidCharacterError: string contains characters outside Latin-1 range".to_string());
        }
        bytes.push(code as u8);
    }
    Ok(BASE64_STANDARD.encode(bytes))
}

fn decode_atob(input: &str) -> Result<String, String> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let padded = match cleaned.len() % 4 {
        0 => cleaned,
        2 => format!("{cleaned}=="),
        3 => format!("{cleaned}="),
        _ => return Err("InvalidCharacterError: invalid base64 string".to_string()),
    };
    let bytes = BASE64_STANDARD
        .decode(padded.as_bytes())
        .map_err(|e| format!("InvalidCharacterError: {e}"))?;
    Ok(bytes.into_iter().map(char::from).collect())
}

fn format_caught_error(err: CaughtError<'_>) -> String {
    match err {
        CaughtError::Exception(exc) => {
            if let Some(msg) = exc.message() {
                msg
            } else {
                format!("{exc}")
            }
        }
        CaughtError::Value(val) => {
            if let Some(s) = val.as_string().and_then(|s| s.to_string().ok()) {
                s
            } else {
                format!("{val:?}")
            }
        }
        CaughtError::Error(e) => e.to_string(),
    }
}

pub fn execute_script(
    code: &str,
    input: ScriptExecutionInput,
    timeout_ms: u64,
) -> ScriptExecutionOutput {
    let mut output = ScriptExecutionOutput {
        updated_globals: input.globals.clone(),
        updated_collection_vars: input.collection_vars.clone(),
        updated_environment_vars: input.environment_vars.clone(),
        updated_local_vars: input.local_vars.clone(),
        ..Default::default()
    };

    if code.trim().is_empty() {
        return output;
    }

    let init_payload = InitPayload {
        request: build_request_payload(&input.request),
        response: input.response.as_ref().map(build_response_payload),
        globals: &input.globals,
        collection_vars: &input.collection_vars,
        environment_vars: &input.environment_vars,
        local_vars: &input.local_vars,
        data_vars: &input.data_vars,
    };

    let init_json = match serde_json::to_string(&init_payload) {
        Ok(json) => json,
        Err(err) => {
            output.error = Some(format!("Failed to serialize script input: {err}"));
            return output;
        }
    };

    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            output.error = Some(format!("Failed to initialize JS runtime: {err}"));
            return output;
        }
    };
    runtime.set_memory_limit(MAX_MEMORY_BYTES);
    runtime.set_max_stack_size(MAX_STACK_BYTES);

    let context = match Context::full(&runtime) {
        Ok(ctx) => ctx,
        Err(err) => {
            output.error = Some(format!("Failed to initialize JS context: {err}"));
            return output;
        }
    };

    let logs: Arc<Mutex<Vec<ConsoleLogEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let tests: Arc<Mutex<Vec<TestResult>>> = Arc::new(Mutex::new(Vec::new()));
    let timed_out = Arc::new(AtomicBool::new(false));

    let logs_ref = Arc::clone(&logs);
    let tests_ref = Arc::clone(&tests);

    let setup_result: Result<(), String> = context.with(|ctx| {
        let globals = ctx.globals();

        globals
            .set("__apitester_init_json", init_json)
            .map_err(|e| format!("Failed to set init state: {e}"))?;

        let log_fn = Function::new(ctx.clone(), move |level: String, message: String| {
            if let Ok(mut guard) = logs_ref.lock() {
                guard.push(ConsoleLogEntry {
                    level: parse_log_level(&level),
                    message,
                    timestamp: Utc::now(),
                });
            }
        })
        .map_err(|e| format!("Failed to bind __host_log: {e}"))?;
        globals
            .set("__host_log", log_fn)
            .map_err(|e| format!("Failed to set __host_log: {e}"))?;

        let test_fn = Function::new(
            ctx.clone(),
            move |name: String, passed: bool, error: String| {
                if let Ok(mut guard) = tests_ref.lock() {
                    guard.push(TestResult {
                        name,
                        passed,
                        error: if passed || error.is_empty() {
                            None
                        } else {
                            Some(error)
                        },
                    });
                }
            },
        )
        .map_err(|e| format!("Failed to bind __host_record_test: {e}"))?;
        globals
            .set("__host_record_test", test_fn)
            .map_err(|e| format!("Failed to set __host_record_test: {e}"))?;

        let btoa_fn = Function::new(ctx.clone(), |ctx: rquickjs::Ctx<'_>, input: String| {
            encode_btoa(&input).map_err(|msg| Exception::throw_message(&ctx, &msg))
        })
        .map_err(|e| format!("Failed to bind __host_btoa: {e}"))?;
        globals
            .set("__host_btoa", btoa_fn)
            .map_err(|e| format!("Failed to set __host_btoa: {e}"))?;

        let atob_fn = Function::new(ctx.clone(), |ctx: rquickjs::Ctx<'_>, input: String| {
            decode_atob(&input).map_err(|msg| Exception::throw_message(&ctx, &msg))
        })
        .map_err(|e| format!("Failed to bind __host_atob: {e}"))?;
        globals
            .set("__host_atob", atob_fn)
            .map_err(|e| format!("Failed to set __host_atob: {e}"))?;

        ctx.eval::<(), _>(PRELUDE_JS)
            .catch(&ctx)
            .map_err(|err| format!("Failed to initialize pm prelude: {}", format_caught_error(err)))?;

        Ok(())
    });

    if let Err(err) = setup_result {
        output.error = Some(err);
        return output;
    }

    if timeout_ms > 0 {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let timed_out_flag = Arc::clone(&timed_out);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if Instant::now() >= deadline {
                timed_out_flag.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        })));
    }

    let exec_error = context.with(|ctx| {
        ctx.eval::<(), _>(code)
            .catch(&ctx)
            .err()
            .map(format_caught_error)
    });

    // Clear interrupt handler before dumping mutated state.
    runtime.set_interrupt_handler(None);

    if timed_out.load(Ordering::Relaxed) {
        output.error = Some(format!("Script execution timed out after {timeout_ms}ms"));
    } else if let Some(err) = exec_error {
        output.error = Some(err);
    }

    let dumped: Option<String> = context.with(|ctx| {
        let dump_fn: Function = ctx.globals().get("__apitester_dump_state").ok()?;
        dump_fn.call::<(), String>(()).ok()
    });

    if let Some(json) = dumped {
        if let Ok(state) = serde_json::from_str::<DumpState>(&json) {
            output.updated_globals = state.globals;
            output.updated_collection_vars = state.collection_vars;
            output.updated_environment_vars = state.environment_vars;
            output.updated_local_vars = state.local_vars;
            output.next_request = state.next_request;
            output.stop_next_request = state.stop_next_request;
        }
    }

    if let Ok(mut guard) = logs.lock() {
        output.console_logs = std::mem::take(&mut *guard);
    }
    if let Ok(mut guard) = tests.lock() {
        output.test_results = std::mem::take(&mut *guard);
    }

    output
}
