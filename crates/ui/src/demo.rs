use apitester_core::models::{
    AuthConfig, Collection, CollectionNode, ConsoleLogEntry, ConsoleLogLevel, Cookie, Environment,
    Folder, HistoryEntry, HttpMethod, KeyValue, ProtocolConfig, RawLanguage, RequestBody,
    RequestItem, ResponseData, ResponseTimings, ScriptConfig, TestResult, Variable,
};
use apitester_proxy::CapturedExchange;
use apitester_runner::{
    CollectionRunReport, LoadTestSummary, LoadTimelinePoint, RequestRunResult,
};
use chrono::Utc;
use uuid::Uuid;

use crate::app::ApiTesterApp;
use crate::state::{LogDirection, RealtimeLogEntry, WorkbenchTab};

/// Populates rich, realistic demo data when `--demo` or `--screenshot-tour` is passed.
pub fn populate_demo_data(app: &mut ApiTesterApp) {
    let mut env_dev = Environment::new("Development");
    env_dev.variables = vec![
        Variable::new("baseUrl", "https://api.dev.apitester.local/v1"),
        Variable::new("apiKey", "dev_pk_99812"),
        Variable::secret("authToken", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.demo"),
    ];
    let dev_id = env_dev.id;

    let mut env_prod = Environment::new("Production");
    env_prod.variables = vec![
        Variable::new("baseUrl", "https://api.apitester.io/v1"),
        Variable::secret("authToken", "prod_secret_token"),
    ];

    app.environments = vec![env_dev, env_prod];
    app.settings.active_environment_id = Some(dev_id);

    // Sample Collection: E-Commerce API
    let mut col = Collection::new("E-Commerce API");
    col.description = "Core REST, GraphQL, and WebSocket endpoints.".into();
    col.auth = AuthConfig::Bearer {
        token: "{{authToken}}".into(),
    };
    col.variables = vec![Variable::new("version", "v1")];

    let mut list_users = RequestItem::new("List Users", HttpMethod::Get, "{{baseUrl}}/users");
    list_users.params = vec![
        KeyValue::new("limit", "25"),
        KeyValue::new("status", "active"),
    ];
    list_users.headers = vec![
        KeyValue::new("Accept", "application/json"),
        KeyValue::new("X-Client", "APITester/0.1"),
    ];
    list_users.scripts = ScriptConfig {
        pre_request: "console.log('Fetching active users...');".into(),
        post_response: concat!(
            "pm.test('Status code is 200', () => {\n",
            "  pm.response.to.have.status(200);\n",
            "});\n",
            "pm.test('Returns user array', () => {\n",
            "  const body = pm.response.json();\n",
            "  pm.expect(body.users).to.have.lengthOf(2);\n",
            "});"
        )
        .into(),
    };

    let mut create_order = RequestItem::new("Create Order", HttpMethod::Post, "{{baseUrl}}/orders");
    create_order.headers = vec![KeyValue::new("Content-Type", "application/json")];
    create_order.body = RequestBody::Raw {
        content: serde_json::json!({
            "customer_id": "usr_42",
            "currency": "USD",
            "items": [
                {"sku": "RUST-BOOK-01", "qty": 1, "price_cents": 3999}
            ]
        })
        .to_string(),
        language: RawLanguage::Json,
    };

    let mut delete_session =
        RequestItem::new("Revoke Session", HttpMethod::Delete, "{{baseUrl}}/sessions/current");
    delete_session.auth = AuthConfig::Inherit;

    let mut gql_catalog = RequestItem::new(
        "Product Catalog (GraphQL)",
        HttpMethod::Post,
        "{{baseUrl}}/graphql",
    );
    gql_catalog.protocol = ProtocolConfig::GraphQL { schema_url: None };
    gql_catalog.body = RequestBody::GraphQL {
        query: "query Products($limit: Int!) {\n  products(limit: $limit) {\n    id\n    name\n    price\n  }\n}".into(),
        variables: r#"{"limit": 10}"#.into(),
    };

    let mut ws_ticker =
        RequestItem::new("Live Order Stream", HttpMethod::Get, "wss://echo.websocket.events");
    ws_ticker.protocol = ProtocolConfig::WebSocket {
        subprotocols: vec!["json".into()],
    };

    let mut users_folder = Folder::new("Users & Orders");
    users_folder.items = vec![
        CollectionNode::Request(list_users.clone()),
        CollectionNode::Request(create_order),
        CollectionNode::Request(delete_session),
    ];

    let mut realtime_folder = Folder::new("GraphQL & Realtime");
    realtime_folder.items = vec![
        CollectionNode::Request(gql_catalog.clone()),
        CollectionNode::Request(ws_ticker.clone()),
    ];

    col.items = vec![
        CollectionNode::Folder(users_folder),
        CollectionNode::Folder(realtime_folder),
    ];
    let col_id = col.id;
    app.collections = vec![col];

    // Pre-populate a rich response on the first open tab
    let sample_body = serde_json::to_vec_pretty(&serde_json::json!({
        "status": "ok",
        "count": 2,
        "users": [
            {"id": 1, "name": "Ada Lovelace", "role": "admin", "active": true},
            {"id": 2, "name": "Grace Hopper", "role": "architect", "active": true}
        ]
    }))
    .unwrap();

    let sample_response = ResponseData {
        status: 200,
        status_text: "OK".into(),
        headers: vec![
            KeyValue::new("content-type", "application/json; charset=utf-8"),
            KeyValue::new("x-request-id", "req_01J9X7K2"),
            KeyValue::new("cache-control", "no-store"),
        ],
        size_bytes: sample_body.len(),
        body: sample_body,
        timings: ResponseTimings {
            dns_ms: 4.2,
            tcp_ms: 9.8,
            tls_ms: 14.5,
            ttfb_ms: 38.4,
            total_ms: 46.1,
        },
        cookies: vec![Cookie {
            name: "session_id".into(),
            value: "s_99a8f1c0".into(),
            domain: "api.dev.apitester.local".into(),
            path: "/".into(),
            expires: None,
            http_only: true,
            secure: true,
            same_site: Some("Lax".into()),
        }],
        test_results: vec![
            TestResult {
                name: "Status code is 200".into(),
                passed: true,
                error: None,
            },
            TestResult {
                name: "Returns user array".into(),
                passed: true,
                error: None,
            },
        ],
        console_logs: vec![
            ConsoleLogEntry {
                level: ConsoleLogLevel::Info,
                message: "> GET https://api.dev.apitester.local/v1/users?limit=25&status=active"
                    .into(),
                timestamp: Utc::now(),
            },
            ConsoleLogEntry {
                level: ConsoleLogLevel::Log,
                message: "Fetching active users...".into(),
                timestamp: Utc::now(),
            },
            ConsoleLogEntry {
                level: ConsoleLogLevel::Info,
                message: "< HTTP 200 OK (46.1 ms)".into(),
                timestamp: Utc::now(),
            },
        ],
    };

    let mut tab1 = WorkbenchTab::from_request(list_users.clone(), Some(col_id));
    tab1.response = Some(sample_response.clone());
    let tab2 = WorkbenchTab::from_request(gql_catalog, Some(col_id));
    let mut tab3 = WorkbenchTab::from_request(ws_ticker, Some(col_id));
    tab3.ws_log = vec![
        RealtimeLogEntry::new(LogDirection::System, "Connected", "wss://echo.websocket.events"),
        RealtimeLogEntry::new(LogDirection::Sent, "Text", r#"{"subscribe":"orders.v1"}"#),
        RealtimeLogEntry::new(
            LogDirection::Received,
            "Text",
            r#"{"event":"order.created","orderId":"ord_8812","total":39.99}"#,
        ),
    ];

    app.tabs = vec![tab1, tab2, tab3];
    app.active_tab = 0;

    app.console_logs = sample_response.console_logs.clone();
    app.cookies = sample_response.cookies.clone();
    app.history = vec![HistoryEntry {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        method: HttpMethod::Get,
        url: "https://api.dev.apitester.local/v1/users?limit=25&status=active".into(),
        status: Some(200),
        duration_ms: Some(46.1),
        request: list_users.clone(),
        response: Some(sample_response.clone()),
    }];

    // Pre-populate sample Runner, Load Tester, and Capture Proxy data for demo/screenshots.
    let run_res = RequestRunResult {
        iteration: 1,
        request_id: list_users.id,
        request_name: list_users.name.clone(),
        method: "GET".into(),
        url: "https://api.dev.apitester.local/v1/users?limit=25&status=active".into(),
        status: Some(200),
        duration_ms: 46.1,
        response: Some(sample_response.clone()),
        test_results: sample_response.test_results.clone(),
        console_logs: Vec::new(),
        error: None,
    };
    app.runner_live_results = vec![run_res.clone()];
    app.runner_report = Some(CollectionRunReport {
        collection_name: "E-Commerce API".into(),
        iterations: 1,
        results: vec![run_res],
        total_requests: 1,
        failed_requests: 0,
        passed_tests: 2,
        failed_tests: 0,
        total_duration_ms: 46.1,
        updated_globals: Default::default(),
        updated_collection_vars: Default::default(),
        updated_environment_vars: Default::default(),
    });

    let timeline: Vec<LoadTimelinePoint> = (1..=10)
        .map(|sec| LoadTimelinePoint {
            second: sec,
            active_vus: (sec as usize * 2).min(16),
            requests: 120 + sec * 18,
            errors: 0,
            avg_latency_ms: 14.0 + sec as f64 * 0.9,
            p95_latency_ms: 28.0 + sec as f64 * 1.8,
        })
        .collect();
    app.load_timeline = timeline.clone();
    app.load_summary = Some(LoadTestSummary {
        total_requests: 2_190,
        error_count: 0,
        error_rate: 0.0,
        rps: 219.0,
        min_ms: 6.2,
        max_ms: 58.4,
        avg_ms: 18.9,
        p50_ms: 16.4,
        p90_ms: 34.2,
        p95_ms: 41.8,
        p99_ms: 53.1,
        duration_secs: 10.0,
        timeline,
    });

    app.captured_exchanges = vec![
        CapturedExchange {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            method: "GET".into(),
            url: "http://api.dev.apitester.local/v1/users?limit=25".into(),
            request_headers: vec![("accept".into(), "application/json".into())],
            request_body: Vec::new(),
            status: 200,
            response_headers: vec![("content-type".into(), "application/json".into())],
            response_body: br#"{"users":[]}"#.to_vec(),
            duration_ms: 19,
        },
        CapturedExchange {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: Utc::now().timestamp_millis(),
            method: "POST".into(),
            url: "http://api.dev.apitester.local/v1/orders".into(),
            request_headers: vec![("content-type".into(), "application/json".into())],
            request_body: br#"{"sku":"RUST-BOOK-01"}"#.to_vec(),
            status: 201,
            response_headers: vec![("content-type".into(), "application/json".into())],
            response_body: br#"{"id":"ord_8812"}"#.to_vec(),
            duration_ms: 34,
        },
    ];
}
