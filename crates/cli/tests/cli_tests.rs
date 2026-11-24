use std::fs;
use std::net::SocketAddr;

use apitester_cli::{execute, Cli};
use apitester_core::models::{
    Collection, CollectionNode, HttpMethod, RequestItem, ScriptConfig, Variable,
};
use apitester_import_export::{export_postman_collection, import_postman_collection};
use clap::Parser;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Minimal HTTP/1.1 mock server for CLI integration tests.
async fn start_mock_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_str.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                let (status, body) = if path.starts_with("/fail") {
                    ("500 Internal Server Error", r#"{"ok":false}"#)
                } else {
                    ("200 OK", r#"{"ok":true,"msg":"hello"}"#)
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    addr
}

#[tokio::test]
async fn cli_run_postman_collection_with_env_and_data_passes() {
    let addr = start_mock_server().await;
    let dir = tempdir().expect("tempdir");

    let mut req = RequestItem::new("Health Check", HttpMethod::Get, "{{baseUrl}}/ok?user={{user}}");
    req.scripts = ScriptConfig {
        pre_request: String::new(),
        post_response: "pm.test('status is 200', () => { pm.response.to.have.status(200); });".into(),
    };

    let mut col = Collection::new("CLI Test Suite");
    col.items.push(CollectionNode::Request(req));
    let pm_json = export_postman_collection(&col).expect("export postman");
    let col_path = dir.path().join("col.postman_collection.json");
    fs::write(&col_path, pm_json).expect("write col");

    let env_json = format!(
        r#"{{"name":"Local","values":[{{"key":"baseUrl","value":"http://{addr}","enabled":true}}]}}"#
    );
    let env_path = dir.path().join("env.json");
    fs::write(&env_path, env_json).expect("write env");

    let csv_path = dir.path().join("data.csv");
    fs::write(&csv_path, "user\nalice\nbob\n").expect("write csv");

    let cli = Cli::try_parse_from([
        "apitester",
        "run",
        col_path.to_str().unwrap(),
        "--env",
        env_path.to_str().unwrap(),
        "--data",
        csv_path.to_str().unwrap(),
        "--iterations",
        "2",
        "--json",
    ])
    .expect("parse run args");

    let out = execute(cli).await.expect("execute run");
    assert_eq!(out.exit_code, 0);

    let report: serde_json::Value = serde_json::from_str(&out.stdout).expect("parse json report");
    assert_eq!(report["total_requests"], 2);
    assert_eq!(report["failed_requests"], 0);
    assert_eq!(report["passed_tests"], 2);
    assert_eq!(report["failed_tests"], 0);
}

#[tokio::test]
async fn cli_run_returns_nonzero_on_failed_test_and_supports_bail() {
    let addr = start_mock_server().await;
    let dir = tempdir().expect("tempdir");

    let mut r1 = RequestItem::new("Failing Req", HttpMethod::Get, format!("http://{addr}/fail"));
    r1.scripts.post_response = "pm.test('expect 200', () => { pm.response.to.have.status(200); });".into();

    let r2 = RequestItem::new("Second Req", HttpMethod::Get, format!("http://{addr}/ok"));

    let mut col = Collection::new("Bail Suite");
    col.items.push(CollectionNode::Request(r1));
    col.items.push(CollectionNode::Request(r2));

    let col_path = dir.path().join("native.json");
    fs::write(&col_path, serde_json::to_string_pretty(&col).unwrap()).expect("write native");

    let cli = Cli::try_parse_from([
        "apitester",
        "run",
        col_path.to_str().unwrap(),
        "--bail",
    ])
    .expect("parse run --bail");

    let out = execute(cli).await.expect("execute run");
    assert_eq!(out.exit_code, 1);
    assert!(out.stdout.contains("[FAIL] expect 200"));
}

#[tokio::test]
async fn cli_load_runs_against_mock_server() {
    let addr = start_mock_server().await;
    let url = format!("http://{addr}/ok");

    let cli = Cli::try_parse_from([
        "apitester",
        "load",
        &url,
        "--method",
        "GET",
        "--vus",
        "2",
        "--duration",
        "1",
        "--profile",
        "constant",
        "--header",
        "X-Test: load",
        "--json",
    ])
    .expect("parse load");

    let out = execute(cli).await.expect("execute load");
    assert_eq!(out.exit_code, 0);

    let summary: serde_json::Value = serde_json::from_str(&out.stdout).expect("parse load json");
    assert!(summary["total_requests"].as_u64().unwrap_or(0) > 0);
    assert_eq!(summary["error_count"], 0);
    assert!(summary["rps"].as_f64().unwrap_or(0.0) > 0.0);
}

#[tokio::test]
async fn cli_import_export_and_codegen() {
    let dir = tempdir().expect("tempdir");

    // 1. Import inline cURL -> file
    let curl_out = dir.path().join("curl_req.json");
    let cli = Cli::try_parse_from([
        "apitester",
        "import",
        "curl -X POST -H 'Content-Type: application/json' -d '{\"a\":1}' https://api.example.com/items",
        "--format",
        "curl",
        "--out",
        curl_out.to_str().unwrap(),
    ])
    .expect("parse import curl");
    let res = execute(cli).await.expect("import curl");
    assert_eq!(res.exit_code, 0);
    let imported_req: RequestItem =
        serde_json::from_str(&fs::read_to_string(&curl_out).unwrap()).expect("parse req json");
    assert_eq!(imported_req.method, HttpMethod::Post);

    // 2. Import OpenAPI YAML -> native collection JSON
    let openapi_path = dir.path().join("spec.yaml");
    fs::write(
        &openapi_path,
        "openapi: '3.0.0'\ninfo:\n  title: Demo\nservers:\n  - url: https://demo.example.com\npaths:\n  /ping:\n    get:\n      summary: Ping\n",
    )
    .unwrap();
    let native_col_path = dir.path().join("demo.json");
    let cli = Cli::try_parse_from([
        "apitester",
        "import",
        openapi_path.to_str().unwrap(),
        "--format",
        "openapi",
        "--out",
        native_col_path.to_str().unwrap(),
    ])
    .expect("parse import openapi");
    execute(cli).await.expect("import openapi");

    // 3. Export native collection -> Postman v2.1 JSON
    let pm_out = dir.path().join("demo.postman.json");
    let cli = Cli::try_parse_from([
        "apitester",
        "export",
        native_col_path.to_str().unwrap(),
        "--out",
        pm_out.to_str().unwrap(),
    ])
    .expect("parse export");
    execute(cli).await.expect("export");
    let reimported =
        import_postman_collection(&fs::read_to_string(&pm_out).unwrap()).expect("valid postman");
    assert_eq!(reimported.name, "Demo");

    // 4. Codegen by request name with collection variables
    let mut col = Collection::new("Codegen Suite");
    col.variables.push(Variable::new("host", "https://api.example.com"));
    col.items.push(CollectionNode::Request(RequestItem::new(
        "List Users",
        HttpMethod::Get,
        "{{host}}/users",
    )));
    let cg_path = dir.path().join("cg.json");
    fs::write(&cg_path, serde_json::to_string_pretty(&col).unwrap()).unwrap();

    for (lang, expected_needle) in [
        ("curl", "curl -X GET 'https://api.example.com/users'"),
        ("rust", "reqwest::Client"),
        ("python", "import requests"),
        ("js", "await fetch("),
        ("go", "http.NewRequest("),
        ("php", "curl_init()"),
    ] {
        let cli = Cli::try_parse_from([
            "apitester",
            "codegen",
            cg_path.to_str().unwrap(),
            "--request",
            "List Users",
            "--lang",
            lang,
        ])
        .expect("parse codegen");
        let out = execute(cli).await.expect("execute codegen");
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.contains(expected_needle),
            "lang {lang} output missing `{expected_needle}`:\n{}",
            out.stdout
        );
    }
}
