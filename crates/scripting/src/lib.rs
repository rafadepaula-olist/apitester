//! Sandboxed JavaScript scripting engine (`pm.*` API) for apitester.

mod engine;

use std::collections::HashMap;

pub use apitester_core::models::ScriptConfig;
use apitester_core::models::{ConsoleLogEntry, RequestItem, ResponseData, TestResult};

/// Input context provided to a Pre-request or Post-response JavaScript script.
#[derive(Debug, Clone, Default)]
pub struct ScriptExecutionInput {
    pub request: RequestItem,
    pub response: Option<ResponseData>,
    pub globals: HashMap<String, String>,
    pub collection_vars: HashMap<String, String>,
    pub environment_vars: HashMap<String, String>,
    pub local_vars: HashMap<String, String>,
    pub data_vars: HashMap<String, String>,
}

/// Output captured after executing a JavaScript script.
#[derive(Debug, Clone, Default)]
pub struct ScriptExecutionOutput {
    pub test_results: Vec<TestResult>,
    pub console_logs: Vec<ConsoleLogEntry>,
    pub updated_globals: HashMap<String, String>,
    pub updated_collection_vars: HashMap<String, String>,
    pub updated_environment_vars: HashMap<String, String>,
    pub updated_local_vars: HashMap<String, String>,
    pub next_request: Option<String>,
    pub stop_next_request: bool,
    pub error: Option<String>,
}

/// Executes a Postman-compatible JavaScript script inside a sandboxed QuickJS runtime.
///
/// If `timeout_ms` is greater than 0, execution is interrupted once the deadline elapses.
pub fn run_script(
    code: &str,
    input: ScriptExecutionInput,
    timeout_ms: u64,
) -> ScriptExecutionOutput {
    engine::execute_script(code, input, timeout_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apitester_core::models::{
        ConsoleLogLevel, HttpMethod, KeyValue, RawLanguage, RequestBody, ResponseTimings,
    };

    fn sample_request() -> RequestItem {
        let mut req = RequestItem::new(
            "Get User",
            HttpMethod::Post,
            "https://api.example.com/users/42",
        );
        req.headers
            .push(KeyValue::new("Content-Type", "application/json"));
        req.headers.push(KeyValue::new("X-Api-Key", "secret-123"));
        req.body = RequestBody::Raw {
            content: r#"{"active":true}"#.to_string(),
            language: RawLanguage::Json,
        };
        req
    }

    fn sample_response() -> ResponseData {
        let body = br#"{"id":42,"name":"Ada Lovelace","active":true,"roles":["admin","editor"],"meta":{"score":98},"empty":null}"#.to_vec();
        ResponseData {
            status: 200,
            status_text: "OK".to_string(),
            headers: vec![
                KeyValue::new("Content-Type", "application/json; charset=utf-8"),
                KeyValue::new("X-Request-Id", "req-999"),
            ],
            size_bytes: body.len(),
            body,
            timings: ResponseTimings {
                total_ms: 42.5,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn pre_request_sets_unsets_and_reads_scoped_variables() {
        let mut input = ScriptExecutionInput {
            request: sample_request(),
            ..Default::default()
        };
        input
            .globals
            .insert("shared".into(), "from-global".into());
        input
            .collection_vars
            .insert("shared".into(), "from-collection".into());
        input
            .environment_vars
            .insert("env_only".into(), "staging".into());
        input
            .data_vars
            .insert("row_id".into(), "row-7".into());

        let code = r#"
            // Verify precedence: data > environment > collection > global
            pm.test("precedence works", () => {
                pm.expect(pm.variables.get("shared")).to.equal("from-collection");
                pm.expect(pm.variables.get("env_only")).to.equal("staging");
                pm.expect(pm.variables.get("row_id")).to.equal("row-7");
                pm.expect(pm.variables.has("shared")).to.be.true;
                pm.expect(pm.variables.has("missing")).to.be.false;
            });

            // Mutate all mutable scopes
            pm.globals.set("g_token", "abc");
            pm.collectionVariables.set("c_ver", 2);
            pm.environment.set("base_url", "https://staging.example.com");
            pm.environment.unset("env_only");
            pm.variables.set("shared", "from-local");

            pm.test("local overrides collection", () => {
                pm.expect(pm.variables.get("shared")).to.equal("from-local");
                pm.expect(pm.environment.has("env_only")).to.be.false;
                pm.expect(pm.globals.toObject()).to.have.property("g_token", "abc");
                pm.expect(pm.collectionVariables.toObject()).to.have.property("c_ver", "2");
                pm.expect(pm.variables.toObject()).to.have.property("shared", "from-local");
            });

            pm.test("pm.request exposes url, method, headers", () => {
                pm.expect(pm.request.url).to.equal("https://api.example.com/users/42");
                pm.expect(pm.request.method).to.equal("POST");
                pm.expect(pm.request.headers.get("x-api-key")).to.equal("secret-123");
                pm.expect(pm.response).to.be.undefined;
            });
        "#;

        let out = run_script(code, input, 2000);
        assert_eq!(out.error, None);
        assert_eq!(out.test_results.len(), 3);
        assert!(out.test_results.iter().all(|t| t.passed));

        assert_eq!(out.updated_globals.get("g_token").map(String::as_str), Some("abc"));
        assert_eq!(
            out.updated_collection_vars.get("c_ver").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            out.updated_environment_vars
                .get("base_url")
                .map(String::as_str),
            Some("https://staging.example.com")
        );
        assert!(!out.updated_environment_vars.contains_key("env_only"));
        assert_eq!(
            out.updated_local_vars.get("shared").map(String::as_str),
            Some("from-local")
        );
    }

    #[test]
    fn post_response_json_parsing_and_response_assertions() {
        let input = ScriptExecutionInput {
            request: sample_request(),
            response: Some(sample_response()),
            ..Default::default()
        };

        let code = r#"
            pm.test("status and headers", () => {
                pm.response.to.have.status(200);
                pm.response.to.have.status("OK");
                pm.response.to.be.ok;
                pm.response.to.be.json;
                pm.expect(pm.response.code).to.equal(200);
                pm.expect(pm.response.status).to.equal("OK");
                pm.expect(pm.response.responseTime).to.be.below(100);
                pm.expect(pm.response.responseTime).to.be.above(10);
                pm.expect(pm.response.headers.get("x-request-id")).to.equal("req-999");
                pm.expect(pm.response.text()).to.include("Ada Lovelace");
            });

            pm.test("chai expect assertions", () => {
                const data = pm.response.json();
                pm.expect(data).to.be.an("object");
                pm.expect(data.id).to.be.a("number").and.to.equal(42);
                pm.expect(data.name).to.be.a("string").and.to.include("Ada");
                pm.expect(data.active).to.be.true;
                pm.expect(!data.active).to.be.false;
                pm.expect(data.empty).to.be.null;
                pm.expect(data.nonexistent).to.be.undefined;
                pm.expect(data.name).to.be.ok;
                pm.expect(data.roles).to.be.an("array").and.to.have.lengthOf(2);
                pm.expect(data.roles).to.include("admin");
                pm.expect(data.roles).to.not.include("guest");
                pm.expect(data).to.have.property("meta");
                pm.expect(data.meta).to.eql({ score: 98 });
                pm.expect(data.meta.score).to.be.above(90).and.to.be.below(100);
                pm.expect(data.id).to.not.equal(99);
            });
        "#;

        let out = run_script(code, input, 2000);
        assert_eq!(out.error, None);
        assert_eq!(out.test_results.len(), 2);
        for res in &out.test_results {
            assert!(res.passed, "test '{}' failed: {:?}", res.name, res.error);
        }
    }

    #[test]
    fn failing_assertions_record_failures_without_stopping_subsequent_tests() {
        let input = ScriptExecutionInput {
            request: sample_request(),
            response: Some(sample_response()),
            ..Default::default()
        };

        let code = r#"
            pm.test("passing test", () => {
                pm.expect(1 + 1).to.equal(2);
            });

            pm.test("failing expect", () => {
                pm.expect(200).to.equal(404);
            });

            pm.test("failing response status", () => {
                pm.response.to.have.status(201);
            });

            pm.test("still runs after failures", () => {
                pm.expect("ok").to.not.be.undefined;
            });
        "#;

        let out = run_script(code, input, 2000);
        assert_eq!(out.error, None);
        assert_eq!(out.test_results.len(), 4);

        assert!(out.test_results[0].passed);
        assert_eq!(out.test_results[0].error, None);

        assert!(!out.test_results[1].passed);
        assert!(out.test_results[1]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("expected 200 to equal 404"));

        assert!(!out.test_results[2].passed);
        assert!(out.test_results[2]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("201"));

        assert!(out.test_results[3].passed);
    }

    #[test]
    fn console_logs_base64_and_set_next_request() {
        let input = ScriptExecutionInput {
            request: sample_request(),
            ..Default::default()
        };

        let code = r#"
            console.log("hello", 123, { a: 1 });
            console.info("info message");
            console.warn("warn message");
            console.error("error message");

            const encoded = btoa("hello:world");
            const decoded = atob(encoded);
            pm.environment.set("b64", encoded);
            pm.environment.set("plain", decoded);

            postman.setNextRequest("Step 2");
            pm.execution.setNextRequest("Step 3");
        "#;

        let out = run_script(code, input, 2000);
        assert_eq!(out.error, None);

        assert_eq!(out.console_logs.len(), 4);
        assert_eq!(out.console_logs[0].level, ConsoleLogLevel::Log);
        assert_eq!(out.console_logs[0].message, r#"hello 123 {"a":1}"#);
        assert_eq!(out.console_logs[1].level, ConsoleLogLevel::Info);
        assert_eq!(out.console_logs[1].message, "info message");
        assert_eq!(out.console_logs[2].level, ConsoleLogLevel::Warn);
        assert_eq!(out.console_logs[2].message, "warn message");
        assert_eq!(out.console_logs[3].level, ConsoleLogLevel::Error);
        assert_eq!(out.console_logs[3].message, "error message");

        assert_eq!(
            out.updated_environment_vars.get("b64").map(String::as_str),
            Some("aGVsbG86d29ybGQ=")
        );
        assert_eq!(
            out.updated_environment_vars.get("plain").map(String::as_str),
            Some("hello:world")
        );
        assert_eq!(out.next_request.as_deref(), Some("Step 3"));
        assert!(!out.stop_next_request);
    }

    #[test]
    fn handles_syntax_errors_runtime_errors_and_timeouts() {
        let syntax_out = run_script("function broken( {", ScriptExecutionInput::default(), 2000);
        assert!(syntax_out.error.is_some());

        let runtime_out = run_script(
            r#"
                console.log("before crash");
                pm.environment.set("saved", "yes");
                nonExistentFunction();
            "#,
            ScriptExecutionInput::default(),
            2000,
        );
        assert!(runtime_out.error.is_some());
        assert_eq!(runtime_out.console_logs.len(), 1);
        assert_eq!(
            runtime_out
                .updated_environment_vars
                .get("saved")
                .map(String::as_str),
            Some("yes")
        );

        let timeout_out = run_script("while (true) {}", ScriptExecutionInput::default(), 50);
        assert!(
            timeout_out
                .error
                .as_deref()
                .unwrap_or("")
                .contains("timed out"),
            "expected timeout error, got: {:?}",
            timeout_out.error
        );
    }

    #[test]
    fn non_ok_non_json_response_assertions_and_edge_cases() {
        let mut input = ScriptExecutionInput {
            request: sample_request(),
            response: Some(ResponseData {
                status: 500,
                status_text: "Internal Server Error".to_string(),
                headers: vec![KeyValue::new("Content-Type", "text/plain")],
                body: b"server exploded".to_vec(),
                size_bytes: 15,
                ..Default::default()
            }),
            ..Default::default()
        };
        input.globals.insert("drop_g".into(), "1".into());
        input.collection_vars.insert("drop_c".into(), "2".into());
        input.local_vars.insert("drop_l".into(), "3".into());

        let code = r#"
            pm.test("500 is not ok", () => {
                pm.response.to.be.ok;
            });
            pm.test("plain text is not json", () => {
                pm.response.to.be.json;
            });
            pm.test("500 is serverError and not ok", () => {
                pm.response.to.be.serverError;
                pm.response.to.not.be.ok;
                pm.response.to.not.have.status(200);
            });
            pm.test("invalid atob throws", () => {
                let threw = false;
                try { atob("%%%invalid%%%"); } catch (e) { threw = true; }
                pm.expect(threw).to.be.true;
            });

            pm.globals.unset("drop_g");
            pm.collectionVariables.unset("drop_c");
            pm.variables.unset("drop_l");
            pm.execution.setNextRequest("Next");
            pm.execution.setNextRequest(null);
        "#;

        let out = run_script(code, input, 2000);
        assert_eq!(out.error, None);
        assert_eq!(out.test_results.len(), 4);
        assert!(!out.test_results[0].passed);
        assert!(!out.test_results[1].passed);
        assert!(out.test_results[2].passed);
        assert!(out.test_results[3].passed);
        assert!(!out.updated_globals.contains_key("drop_g"));
        assert!(!out.updated_collection_vars.contains_key("drop_c"));
        assert!(!out.updated_local_vars.contains_key("drop_l"));
        assert_eq!(out.next_request, None);
        assert!(out.stop_next_request);
    }

    #[test]
    fn empty_script_is_noop_and_preserves_vars() {
        let mut input = ScriptExecutionInput::default();
        input.environment_vars.insert("keep".into(), "val".into());

        let out = run_script("   \n  ", input, 1000);
        assert_eq!(out.error, None);
        assert!(out.test_results.is_empty());
        assert!(out.console_logs.is_empty());
        assert_eq!(out.next_request, None);
        assert!(!out.stop_next_request);
        assert_eq!(
            out.updated_environment_vars.get("keep").map(String::as_str),
            Some("val")
        );
    }

    #[test]
    fn dump_state_cannot_be_overwritten_and_host_logs_are_bounded() {
        let input = ScriptExecutionInput::default();
        let code = r#"
            try {
                globalThis.__apitester_dump_state = function() { while (true) {} };
            } catch (_) {}
            for (var i = 0; i < 1500; i++) {
                console.log("msg-" + i);
            }
        "#;
        let out = run_script(code, input, 1000);
        assert_eq!(out.error, None);
        assert!(out.console_logs.len() <= 1_000);
    }
}
