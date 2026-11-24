//! Headless CLI for apitester: collection runner, load tester, import/export, codegen, and capture proxy.

use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use apitester_core::models::{
    AuthConfig, Collection, CollectionNode, Environment, HttpMethod, KeyValue, RawLanguage,
    RequestBody, RequestItem,
};
use apitester_core::variables::{VariableContext, VariableScope};
use apitester_import_export::{
    export_postman_collection, generate_code, import_curl, import_har, import_openapi,
    import_postman_collection, import_postman_environment, CodegenLanguage, ImportExportError,
};
use apitester_proxy::{CaptureProxyServer, ProxyError};
use apitester_runner::{
    parse_csv_data, parse_json_data, run_collection, run_load_test, CollectionRunReport,
    LoadProfile, LoadTestConfig, LoadTestSummary, RunnerError, RunnerOptions,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use thiserror::Error;

/// Errors returned by CLI operations.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("import/export error: {0}")]
    ImportExport(#[from] ImportExportError),

    #[error("runner error: {0}")]
    Runner(#[from] RunnerError),

    #[error("proxy error: {0}")]
    Proxy(#[from] ProxyError),

    #[error("{0}")]
    Usage(String),
}

/// Result of executing a CLI command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub exit_code: i32,
}

impl CommandOutput {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            exit_code: 0,
        }
    }
}

/// Top-level CLI arguments.
#[derive(Debug, Parser)]
#[command(
    name = "apitester",
    version,
    about = "Newman-compatible headless API collection runner, load tester, and converter"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run a collection (Postman v2.1 or native JSON) with optional environment and data file.
    Run(RunArgs),
    /// Run a native async HTTP load test against a URL.
    Load(LoadArgs),
    /// Import Postman, cURL, OpenAPI/Swagger, or HAR into native apitester JSON.
    Import(ImportArgs),
    /// Export a collection to Postman Collection v2.1.0 JSON.
    Export(ExportArgs),
    /// Generate an HTTP client code snippet for a request in a collection.
    Codegen(CodegenArgs),
    /// Start a local HTTP/HTTPS capture proxy and print captured exchanges until Ctrl+C.
    Proxy(ProxyArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Path to collection JSON (Postman v2.1 or native apitester format).
    pub collection: PathBuf,

    /// Path to environment JSON (Postman or native apitester format).
    #[arg(long)]
    pub env: Option<PathBuf>,

    /// Path to iteration data file (CSV or JSON array of objects).
    #[arg(long)]
    pub data: Option<PathBuf>,

    /// Number of iterations to run.
    #[arg(long, default_value_t = 1)]
    pub iterations: usize,

    /// Delay in milliseconds between requests.
    #[arg(long, default_value_t = 0)]
    pub delay: u64,

    /// Stop the run immediately on the first request error or failed test assertion.
    #[arg(long)]
    pub bail: bool,

    /// Emit full machine-readable JSON report.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LoadProfileArg {
    Constant,
    Ramp,
    Spike,
    Peak,
}

#[derive(Debug, Args)]
pub struct LoadArgs {
    /// Target URL to load test.
    pub url: String,

    /// HTTP method (GET, POST, PUT, PATCH, DELETE, etc.).
    #[arg(short = 'X', long, default_value = "GET")]
    pub method: String,

    /// Number of virtual users (target/peak concurrency).
    #[arg(long, default_value_t = 10)]
    pub vus: usize,

    /// Test duration in seconds.
    #[arg(long, default_value_t = 10)]
    pub duration: u64,

    /// Concurrency load profile.
    #[arg(long, value_enum, default_value_t = LoadProfileArg::Constant)]
    pub profile: LoadProfileArg,

    /// Request header in `"Key: Value"` format (repeatable).
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// Optional raw request body.
    #[arg(long)]
    pub body: Option<String>,

    /// Emit full machine-readable JSON summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ImportFormatArg {
    Postman,
    Curl,
    Openapi,
    Har,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Input file path (or inline cURL command when `--format curl` and path does not exist).
    pub input: String,

    /// Source format to import from.
    #[arg(long, value_enum)]
    pub format: ImportFormatArg,

    /// Optional output file path (writes to stdout if omitted).
    #[arg(short, long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Path to collection JSON (native apitester or Postman v2.1).
    pub collection: PathBuf,

    /// Optional output file path (writes to stdout if omitted).
    #[arg(short, long)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CodegenLangArg {
    Curl,
    Rust,
    Python,
    Js,
    Go,
    Php,
}

impl From<CodegenLangArg> for CodegenLanguage {
    fn from(value: CodegenLangArg) -> Self {
        match value {
            CodegenLangArg::Curl => Self::Curl,
            CodegenLangArg::Rust => Self::RustReqwest,
            CodegenLangArg::Python => Self::PythonRequests,
            CodegenLangArg::Js => Self::JavaScriptFetch,
            CodegenLangArg::Go => Self::GoNetHttp,
            CodegenLangArg::Php => Self::PhpCurl,
        }
    }
}

#[derive(Debug, Args)]
pub struct CodegenArgs {
    /// Path to collection JSON (native apitester or Postman v2.1).
    pub collection: PathBuf,

    /// Request name or 0-based index within the flattened collection (default: 0).
    #[arg(long, default_value = "0")]
    pub request: String,

    /// Target code snippet language.
    #[arg(long, value_enum, default_value_t = CodegenLangArg::Curl)]
    pub lang: CodegenLangArg,

    /// Optional environment JSON file for variable interpolation.
    #[arg(long)]
    pub env: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProxyArgs {
    /// Socket address to bind the capture proxy to.
    #[arg(long, default_value = "127.0.0.1:8888")]
    pub bind: SocketAddr,
}

/// Executes a parsed [`Cli`] command and returns its output and exit code.
pub async fn execute(cli: Cli) -> Result<CommandOutput, CliError> {
    match cli.command {
        Commands::Run(args) => cmd_run(args).await,
        Commands::Load(args) => cmd_load(args).await,
        Commands::Import(args) => cmd_import(args),
        Commands::Export(args) => cmd_export(args),
        Commands::Codegen(args) => cmd_codegen(args),
        Commands::Proxy(args) => cmd_proxy(args).await,
    }
}

/// Loads a [`Collection`] from disk, auto-detecting Postman v2.1 JSON vs native apitester JSON.
pub fn load_collection(path: &Path) -> Result<Collection, CliError> {
    let raw = fs::read_to_string(path)?;
    parse_collection_str(&raw)
}

/// Parses a [`Collection`] JSON string, auto-detecting Postman v2.1 vs native apitester format.
pub fn parse_collection_str(raw: &str) -> Result<Collection, CliError> {
    let val: serde_json::Value = serde_json::from_str(raw)?;
    let is_postman = val
        .as_object()
        .is_some_and(|o| o.contains_key("info") && o.contains_key("item"));

    if is_postman {
        Ok(import_postman_collection(raw)?)
    } else {
        Ok(serde_json::from_value(val)?)
    }
}

/// Loads an [`Environment`] from disk, auto-detecting Postman environment JSON vs native JSON.
pub fn load_environment(path: &Path) -> Result<Environment, CliError> {
    let raw = fs::read_to_string(path)?;
    let val: serde_json::Value = serde_json::from_str(&raw)?;
    let is_postman = val.as_object().is_some_and(|o| o.contains_key("values"));

    if is_postman {
        Ok(import_postman_environment(&raw)?)
    } else {
        Ok(serde_json::from_value(val)?)
    }
}

async fn cmd_run(args: RunArgs) -> Result<CommandOutput, CliError> {
    let collection = load_collection(&args.collection)?;

    let mut vars = VariableContext::new();
    vars.load_variables(VariableScope::Collection, &collection.variables);

    if let Some(env_path) = &args.env {
        let env = load_environment(env_path)?;
        vars.load_variables(VariableScope::Environment, &env.variables);
    }

    let data_rows = if let Some(data_path) = &args.data {
        let raw = fs::read_to_string(data_path)?;
        let is_json = data_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            || raw.trim_start().starts_with('[');
        if is_json {
            parse_json_data(&raw)?
        } else {
            parse_csv_data(&raw)?
        }
    } else {
        Vec::new()
    };

    let options = RunnerOptions {
        iterations: args.iterations.max(1),
        delay_ms: args.delay,
        data_rows,
        stop_on_error: args.bail,
        ..RunnerOptions::default()
    };

    let report = run_collection(&collection, vars, options, None).await;
    let failed = report.failed_requests > 0 || report.failed_tests > 0;
    let stdout = if args.json {
        serde_json::to_string_pretty(&report)?
    } else {
        format_run_report(&report)
    };

    Ok(CommandOutput {
        stdout,
        exit_code: if failed { 1 } else { 0 },
    })
}

fn format_run_report(report: &CollectionRunReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Collection: {} ({} iteration(s))",
        report.collection_name, report.iterations
    );

    for res in &report.results {
        let status_str = res
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "ERR".to_string());
        let _ = writeln!(
            out,
            "  [iter {}] {} {} -> {} ({:.1} ms) [{}]",
            res.iteration + 1,
            res.method,
            res.url,
            status_str,
            res.duration_ms,
            res.request_name
        );
        if let Some(err) = &res.error {
            let _ = writeln!(out, "    ERROR: {err}");
        }
        for test in &res.test_results {
            let mark = if test.passed { "PASS" } else { "FAIL" };
            if let Some(err) = &test.error {
                let _ = writeln!(out, "    [{mark}] {}: {err}", test.name);
            } else {
                let _ = writeln!(out, "    [{mark}] {}", test.name);
            }
        }
    }

    let _ = writeln!(
        out,
        "Summary: {} requests ({} failed), {} tests passed, {} tests failed, {:.1} ms total",
        report.total_requests,
        report.failed_requests,
        report.passed_tests,
        report.failed_tests,
        report.total_duration_ms
    );
    out
}

async fn cmd_load(args: LoadArgs) -> Result<CommandOutput, CliError> {
    let method = HttpMethod::from_str(&args.method).unwrap_or(HttpMethod::Get);
    let mut req = RequestItem::new("Load Target", method, args.url);

    for raw_header in &args.headers {
        let Some((k, v)) = raw_header.split_once(':') else {
            return Err(CliError::Usage(format!(
                "invalid header `{raw_header}`: expected `Key: Value`"
            )));
        };
        req.headers.push(KeyValue::new(k.trim(), v.trim()));
    }

    if let Some(body) = args.body {
        let is_json = req
            .headers
            .iter()
            .any(|h| h.key.eq_ignore_ascii_case("content-type") && h.value.contains("json"))
            || body.trim_start().starts_with('{')
            || body.trim_start().starts_with('[');
        req.body = RequestBody::Raw {
            content: body,
            language: if is_json {
                RawLanguage::Json
            } else {
                RawLanguage::Text
            },
        };
    }

    let vus = args.vus.max(1);
    let profile = match args.profile {
        LoadProfileArg::Constant => LoadProfile::Constant { vus },
        LoadProfileArg::Ramp => LoadProfile::Ramp {
            start_vus: 1,
            end_vus: vus,
        },
        LoadProfileArg::Spike => LoadProfile::Spike {
            base_vus: (vus / 4).max(1),
            spike_vus: vus,
            spike_at_pct: 0.4,
            spike_duration_pct: 0.2,
        },
        LoadProfileArg::Peak => LoadProfile::Peak {
            base_vus: 1,
            peak_vus: vus,
        },
    };

    let config = LoadTestConfig {
        duration_secs: args.duration.max(1),
        profile,
        ..LoadTestConfig::default()
    };

    let summary = run_load_test(req, VariableContext::new(), AuthConfig::NoAuth, config, None).await;
    let stdout = if args.json {
        serde_json::to_string_pretty(&summary)?
    } else {
        format_load_summary(&summary)
    };

    Ok(CommandOutput::ok(stdout))
}

fn format_load_summary(s: &LoadTestSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Load Test Summary ({:.2}s):", s.duration_secs);
    let _ = writeln!(
        out,
        "  Requests: {} | Errors: {} ({:.2}%) | RPS: {:.2}",
        s.total_requests,
        s.error_count,
        s.error_rate * 100.0,
        s.rps
    );
    let _ = writeln!(
        out,
        "  Latency (ms): min={:.2} avg={:.2} p50={:.2} p90={:.2} p95={:.2} p99={:.2} max={:.2}",
        s.min_ms, s.avg_ms, s.p50_ms, s.p90_ms, s.p95_ms, s.p99_ms, s.max_ms
    );
    out
}

fn cmd_import(args: ImportArgs) -> Result<CommandOutput, CliError> {
    let input_path = Path::new(&args.input);
    let content = if input_path.exists() {
        fs::read_to_string(input_path)?
    } else if matches!(args.format, ImportFormatArg::Curl) {
        args.input.clone()
    } else {
        return Err(CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("input file not found: {}", args.input),
        )));
    };

    let json = match args.format {
        ImportFormatArg::Postman => {
            let col = import_postman_collection(&content)?;
            serde_json::to_string_pretty(&col)?
        }
        ImportFormatArg::Curl => {
            let req = import_curl(&content)?;
            serde_json::to_string_pretty(&req)?
        }
        ImportFormatArg::Openapi => {
            let col = import_openapi(&content)?;
            serde_json::to_string_pretty(&col)?
        }
        ImportFormatArg::Har => {
            let col = import_har(&content)?;
            serde_json::to_string_pretty(&col)?
        }
    };

    if let Some(out_path) = args.out {
        fs::write(&out_path, &json)?;
        Ok(CommandOutput::ok(format!(
            "Wrote imported JSON to {}\n",
            out_path.display()
        )))
    } else {
        Ok(CommandOutput::ok(format!("{json}\n")))
    }
}

fn cmd_export(args: ExportArgs) -> Result<CommandOutput, CliError> {
    let collection = load_collection(&args.collection)?;
    let postman_json = export_postman_collection(&collection)?;

    if let Some(out_path) = args.out {
        fs::write(&out_path, &postman_json)?;
        Ok(CommandOutput::ok(format!(
            "Exported Postman v2.1 collection to {}\n",
            out_path.display()
        )))
    } else {
        Ok(CommandOutput::ok(format!("{postman_json}\n")))
    }
}

fn cmd_codegen(args: CodegenArgs) -> Result<CommandOutput, CliError> {
    let collection = load_collection(&args.collection)?;

    let mut vars = VariableContext::new();
    vars.load_variables(VariableScope::Collection, &collection.variables);
    if let Some(env_path) = &args.env {
        let env = load_environment(env_path)?;
        vars.load_variables(VariableScope::Environment, &env.variables);
    }

    let requests = flatten_requests(&collection.items);
    if requests.is_empty() {
        return Err(CliError::Usage("collection contains no requests".into()));
    }

    let target = select_request(&requests, &args.request).ok_or_else(|| {
        CliError::Usage(format!(
            "request `{}` not found in collection `{}`",
            args.request, collection.name
        ))
    })?;

    let snippet = generate_code(target, args.lang.into(), &vars);
    Ok(CommandOutput::ok(snippet))
}

fn flatten_requests(nodes: &[CollectionNode]) -> Vec<&RequestItem> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            CollectionNode::Request(r) => out.push(r),
            CollectionNode::Folder(f) => out.extend(flatten_requests(&f.items)),
        }
    }
    out
}

fn select_request<'a>(requests: &[&'a RequestItem], selector: &str) -> Option<&'a RequestItem> {
    if let Ok(idx) = selector.parse::<usize>() {
        if let Some(&req) = requests.get(idx) {
            return Some(req);
        }
    }
    if let Some(&req) = requests.iter().find(|r| r.name == selector) {
        return Some(req);
    }
    let lower = selector.to_ascii_lowercase();
    requests
        .iter()
        .find(|r| r.name.to_ascii_lowercase().contains(&lower))
        .copied()
}

async fn cmd_proxy(args: ProxyArgs) -> Result<CommandOutput, CliError> {
    let (server, addr, mut rx) = CaptureProxyServer::start(args.bind).await?;
    println!("Capture proxy listening on http://{addr} (press Ctrl+C to stop)");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                server.stop();
                break;
            }
            recv = rx.recv() => {
                match recv {
                    Ok(ex) => {
                        println!(
                            "[{}] {} {} -> {} ({} ms)",
                            ex.timestamp_ms, ex.method, ex.url, ex.status, ex.duration_ms
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("proxy receiver lagged by {n} message(s)");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(CommandOutput::ok("Capture proxy stopped.\n".to_string()))
}
