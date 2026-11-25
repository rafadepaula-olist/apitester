use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use apitester_auth::{apply_auth, AuthRequestContext};
use apitester_core::models::{
    resolve_auth, AppSettings, AuthConfig, Collection, CollectionNode, ConsoleLogEntry,
    ConsoleLogLevel, Cookie, Environment, Folder, HistoryEntry, HttpMethod, KeyValue,
    ProtocolConfig, RawLanguage, RequestBody, RequestItem, SavedExample, ThemeMode, Variable,
};
use apitester_core::storage::{JsonStore, SqliteStore};
use apitester_core::variables::{VariableContext, VariableResolver, VariableScope};
use apitester_import_export::CodegenLanguage;
use apitester_net::{introspect_schema, HttpClient, HttpRequestOptions};
use apitester_proxy::{CaptureProxyServer, CapturedExchange};
use apitester_runner::{
    CollectionRunReport, LoadTestSummary, LoadTimelinePoint, RequestRunResult, RunnerProgressEvent,
};
use apitester_scripting::{run_script, ScriptExecutionInput};
use chrono::Utc;
use egui::{
    CentralPanel, Color32, ComboBox, Frame, Id, Key, Margin, Modifiers, RichText, Rounding,
    ScrollArea, SidePanel, Stroke, TopBottomPanel, Ui, Vec2,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::demo::populate_demo_data;
use crate::screenshot_tour::ScreenshotTour;
use crate::state::{
    AppTab, ImportFormat, LoadProfileKind, RequestSubTab, ResponseBodyView, ResponseSubTab,
    RunnerModalTab, SidebarTab, UiMessage, WorkbenchTab,
};
use crate::theme::{
    apply_theme, ghost_button, highlight_body, load_system_fonts, method_badge,
    method_color_for_mode, primary_button, status_badge, status_color_for_mode, subtle_divider,
    underline_tab, Palette,
};
use crate::widgets::auth_editor::auth_editor_ui;
use crate::widgets::kv_table::{form_data_table_ui, kv_table_ui, variables_table_ui};
use crate::widgets::modals::{
    import_from_file_path, render_capture_proxy_tab, render_code_drawer,
    render_collection_settings_tab, render_import_export_tab, render_runner_tab,
    render_settings_tab,
};
use crate::widgets::protocol_views::protocol_workbench_ui;

/// CLI launch options for `ApiTesterApp`.
#[derive(Debug, Clone, Default)]
pub struct LaunchConfig {
    pub demo_mode: bool,
    pub data_dir: Option<PathBuf>,
    pub screenshot_dir: Option<PathBuf>,
}

/// Main `eframe::App` state for APITester.
pub struct ApiTesterApp {
    pub rt: Handle,
    pub tx: UnboundedSender<UiMessage>,
    rx: UnboundedReceiver<UiMessage>,

    pub json_store: JsonStore,
    pub sqlite_store: SqliteStore,

    pub settings: AppSettings,
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub history: Vec<HistoryEntry>,
    pub cookies: Vec<Cookie>,

    pub tabs: Vec<AppTab>,
    pub active_tab: usize,

    pub sidebar_open: bool,
    pub sidebar_tab: SidebarTab,
    pub search_query: String,
    pub reveal_secrets: bool,
    pub console_open: bool,
    pub console_logs: Vec<ConsoleLogEntry>,

    // Collapsible right live Code Snippet drawer
    pub code_drawer_open: bool,
    pub code_drawer_lang: CodegenLanguage,

    // Capture proxy state
    pub proxy_port: u16,
    pub proxy_status: String,
    pub proxy_server: Option<CaptureProxyServer>,
    pub captured_exchanges: Vec<CapturedExchange>,

    // Runner & Load Tester state
    pub runner_modal_tab: RunnerModalTab,
    pub runner_iterations: usize,
    pub runner_delay_ms: u64,
    pub runner_stop_on_error: bool,
    pub runner_data_input: String,
    pub runner_in_flight: bool,
    pub runner_live_results: Vec<RequestRunResult>,
    pub runner_report: Option<CollectionRunReport>,

    pub load_profile_kind: LoadProfileKind,
    pub load_vus: usize,
    pub load_peak_vus: usize,
    pub load_duration_secs: u64,
    pub load_in_flight: bool,
    pub load_cancel: Option<Arc<AtomicBool>>,
    pub load_timeline: Vec<LoadTimelinePoint>,
    pub load_summary: Option<LoadTestSummary>,

    pub screenshot_tour: Option<ScreenshotTour>,
    focus_url_next: bool,
    focus_search_next: bool,
    theme_applied: Option<ThemeMode>,
}

impl ApiTesterApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: Handle,
        config: LaunchConfig,
    ) -> Self {
        load_system_fonts(&cc.egui_ctx);
        let mut app = Self::new_headless(rt, config);
        apply_theme(&cc.egui_ctx, app.settings.theme);
        app.theme_applied = Some(app.settings.theme);
        app
    }

    /// Creates an `ApiTesterApp` without requiring a GPU/window context (used in unit tests).
    pub fn new_headless(rt: Handle, config: LaunchConfig) -> Self {
        let data_dir = config.data_dir.unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("apitester")
        });

        let json_store = JsonStore::open(&data_dir).unwrap_or_else(|_| {
            JsonStore::open(std::env::temp_dir().join("apitester_fallback"))
                .expect("open fallback json store")
        });
        let sqlite_store = SqliteStore::open(data_dir.join("apitester.sqlite"))
            .unwrap_or_else(|_| SqliteStore::open_in_memory().expect("open in-memory sqlite"));

        let settings = sqlite_store.load_settings().unwrap_or_default();
        let collections = json_store.list_collections().unwrap_or_default();
        let environments = json_store.list_environments().unwrap_or_default();
        let history = sqlite_store.list_history(200, 0).unwrap_or_default();
        let cookies = sqlite_store.list_cookies().unwrap_or_default();

        let (tx, rx) = mpsc::unbounded_channel();
        let screenshot_tour = config.screenshot_dir.map(ScreenshotTour::new);

        let mut app = Self {
            rt,
            tx,
            rx,
            json_store,
            sqlite_store,
            settings,
            collections,
            environments,
            history,
            cookies,
            tabs: vec![AppTab::Request(Box::new(WorkbenchTab::new_protocol(
                ProtocolConfig::Http,
            )))],
            active_tab: 0,
            sidebar_open: true,
            sidebar_tab: SidebarTab::Collections,
            search_query: String::new(),
            reveal_secrets: false,
            console_open: false,
            console_logs: Vec::new(),
            code_drawer_open: false,
            code_drawer_lang: CodegenLanguage::Curl,
            proxy_port: 8888,
            proxy_status: "Stopped".into(),
            proxy_server: None,
            captured_exchanges: Vec::new(),
            runner_modal_tab: RunnerModalTab::CollectionRunner,
            runner_iterations: 1,
            runner_delay_ms: 0,
            runner_stop_on_error: false,
            runner_data_input: String::new(),
            runner_in_flight: false,
            runner_live_results: Vec::new(),
            runner_report: None,
            load_profile_kind: LoadProfileKind::Ramp,
            load_vus: 4,
            load_peak_vus: 16,
            load_duration_secs: 10,
            load_in_flight: false,
            load_cancel: None,
            load_timeline: Vec::new(),
            load_summary: None,
            screenshot_tour,
            focus_url_next: false,
            focus_search_next: false,
            theme_applied: None,
        };

        if config.demo_mode {
            populate_demo_data(&mut app);
        }

        app
    }

    /// Builds a scoped `VariableContext` from globals, collection vars, and active environment.
    pub fn build_variable_context(&self, collection_id: Option<Uuid>) -> VariableContext {
        let mut ctx = VariableContext::new();
        ctx.load_variables(VariableScope::Global, &self.settings.global_variables);

        if let Some(cid) = collection_id {
            if let Some(col) = self.collections.iter().find(|c| c.id == cid) {
                ctx.load_variables(VariableScope::Collection, &col.variables);
            }
        }

        if let Some(eid) = self.settings.active_environment_id {
            if let Some(env) = self.environments.iter().find(|e| e.id == eid) {
                ctx.load_variables(VariableScope::Environment, &env.variables);
            }
        }

        ctx
    }

    pub fn open_or_focus_request(&mut self, req: RequestItem, collection_id: Option<Uuid>) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.as_request().is_some_and(|r| r.request.id == req.id))
        {
            self.active_tab = idx;
            return;
        }
        self.tabs
            .push(AppTab::Request(Box::new(WorkbenchTab::from_request(req, collection_id))));
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn open_or_focus_runner_tab(&mut self) {
        if let Some(idx) = self.tabs.iter().position(|t| matches!(t, AppTab::Runner)) {
            self.active_tab = idx;
        } else {
            self.tabs.push(AppTab::Runner);
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn open_or_focus_import_tab(&mut self) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| matches!(t, AppTab::ImportExport { .. }))
        {
            self.active_tab = idx;
        } else {
            self.tabs.push(AppTab::ImportExport {
                format: ImportFormat::PostmanCollection,
                file_path: String::new(),
                input_text: String::new(),
                status: String::new(),
            });
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn open_or_focus_proxy_tab(&mut self) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| matches!(t, AppTab::CaptureProxy))
        {
            self.active_tab = idx;
        } else {
            self.tabs.push(AppTab::CaptureProxy);
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn open_or_focus_settings_tab(&mut self) {
        if let Some(idx) = self.tabs.iter().position(|t| matches!(t, AppTab::Settings)) {
            self.active_tab = idx;
        } else {
            self.tabs.push(AppTab::Settings);
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn open_or_focus_collection_settings(&mut self, collection_id: Uuid) {
        if let Some(idx) = self.tabs.iter().position(|t| {
            matches!(
                t,
                AppTab::CollectionSettings {
                    collection_id: cid
                } if *cid == collection_id
            )
        }) {
            self.active_tab = idx;
        } else {
            self.tabs.push(AppTab::CollectionSettings { collection_id });
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.tabs.push(AppTab::Request(Box::new(WorkbenchTab::new_protocol(
                ProtocolConfig::Http,
            ))));
            self.active_tab = 0;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn toggle_theme(&mut self) {
        self.settings.theme = match self.settings.theme {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark | ThemeMode::System => ThemeMode::Light,
        };
        let _ = self.sqlite_store.save_settings(&self.settings);
    }

    fn find_request_tab_mut(&mut self, tab_id: Uuid) -> Option<&mut WorkbenchTab> {
        self.tabs
            .iter_mut()
            .filter_map(AppTab::as_request_mut)
            .find(|t| t.id == tab_id)
    }

    pub fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                UiMessage::HttpFinished {
                    tab_id,
                    result,
                    updated_globals,
                    updated_env,
                    updated_collection,
                    extra_logs,
                } => {
                    self.apply_updated_vars(updated_globals, updated_env, updated_collection);
                    self.console_logs.extend(extra_logs.clone());

                    let mut new_history = None;
                    let mut new_cookies = Vec::new();
                    let mut resp_logs = Vec::new();
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.in_flight = false;
                        tab.cancel_handle = None;
                        match result {
                            Ok(mut resp) => {
                                resp.console_logs.extend(extra_logs);
                                resp_logs = resp.console_logs.clone();
                                new_cookies = resp.cookies.clone();
                                new_history = Some(HistoryEntry {
                                    id: Uuid::new_v4(),
                                    timestamp: Utc::now(),
                                    method: tab.request.method.clone(),
                                    url: tab.request.url.clone(),
                                    status: Some(resp.status),
                                    duration_ms: Some(resp.timings.total_ms),
                                    request: tab.request.clone(),
                                    response: Some(resp.clone()),
                                });
                                tab.last_error = None;
                                tab.response = Some(resp);
                            }
                            Err(err) => {
                                tab.last_error = Some(err);
                            }
                        }
                    }

                    self.console_logs.extend(resp_logs);

                    for cookie in &new_cookies {
                        let _ = self.sqlite_store.upsert_cookie(cookie);
                    }
                    if !new_cookies.is_empty() {
                        self.cookies = self.sqlite_store.list_cookies().unwrap_or_default();
                    }
                    if let Some(entry) = new_history {
                        let _ = self.sqlite_store.insert_history(&entry);
                        self.history.insert(0, entry);
                    }
                }
                UiMessage::GraphQlIntrospected { tab_id, result } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        match result {
                            Ok(schema) => tab.graphql_schema_json = Some(schema),
                            Err(err) => tab.last_error = Some(err),
                        }
                    }
                }
                UiMessage::WsConnected { tab_id, client } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.ws_client = Some(client);
                        tab.ws_connected = true;
                    }
                }
                UiMessage::WsLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.ws_log.push(entry);
                        if disconnected {
                            tab.ws_connected = false;
                            tab.ws_client = None;
                        }
                    }
                }
                UiMessage::GrpcFinished { tab_id, result } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.grpc_response = Some(match result {
                            Ok(s) => s,
                            Err(e) => format!("Error: {e}"),
                        });
                    }
                }
                UiMessage::MqttConnected { tab_id, client } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.mqtt_client = Some(client);
                        tab.mqtt_connected = true;
                    }
                }
                UiMessage::MqttLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.mqtt_log.push(entry);
                        if disconnected {
                            tab.mqtt_connected = false;
                            tab.mqtt_client = None;
                        }
                    }
                }
                UiMessage::SocketIoConnected { tab_id, client } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.sio_client = Some(client);
                        tab.sio_connected = true;
                    }
                }
                UiMessage::SocketIoLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.sio_log.push(entry);
                        if disconnected {
                            tab.sio_connected = false;
                            tab.sio_client = None;
                        }
                    }
                }
                UiMessage::McpOutput {
                    tab_id,
                    output,
                    tools,
                    resources,
                } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.mcp_output = output;
                        if let Some(t) = tools {
                            tab.mcp_tools = t;
                        }
                        if let Some(r) = resources {
                            tab.mcp_resources = r;
                        }
                    }
                }
                UiMessage::SseLog {
                    tab_id,
                    entry,
                    ended,
                } => {
                    if let Some(tab) = self.find_request_tab_mut(tab_id) {
                        tab.sse_log.push(entry);
                        if ended {
                            tab.sse_connected = false;
                            tab.sse_cancel = None;
                        }
                    }
                }
                UiMessage::ProxyCaptured(ex) => {
                    self.captured_exchanges.insert(0, ex);
                }
                UiMessage::RunnerProgress(ev) => {
                    if let RunnerProgressEvent::RequestCompleted { result, .. } = ev {
                        self.runner_live_results.push(*result);
                    }
                }
                UiMessage::RunnerFinished(report) => {
                    self.runner_in_flight = false;
                    self.runner_live_results = report.results.clone();
                    self.runner_report = Some(report);
                }
                UiMessage::LoadProgress(update) => {
                    self.load_timeline.push(update.latest_bucket);
                }
                UiMessage::LoadFinished(summary) => {
                    self.load_in_flight = false;
                    self.load_cancel = None;
                    self.load_timeline = summary.timeline.clone();
                    self.load_summary = Some(summary);
                }
                UiMessage::ImportFilePicked(res) => match res {
                    Ok(Some(path)) => {
                        let (detected, text, msg) = import_from_file_path(self, &path);
                        self.open_or_focus_import_tab();
                        if let Some(AppTab::ImportExport {
                            format,
                            file_path,
                            input_text,
                            status,
                        }) = self.tabs.get_mut(self.active_tab)
                        {
                            *format = detected;
                            *file_path = path.display().to_string();
                            *input_text = text;
                            *status = msg;
                        }
                    }
                    Ok(None) => {
                        for tab in &mut self.tabs {
                            if let AppTab::ImportExport { status, .. } = tab {
                                *status = "File selection cancelled.".into();
                            }
                        }
                    }
                    Err(err) => {
                        for tab in &mut self.tabs {
                            if let AppTab::ImportExport { status, .. } = tab {
                                *status = format!("File dialog error: {err}");
                            }
                        }
                    }
                },
                UiMessage::ExportFileSaved(res) => {
                    let msg = match res {
                        Ok(Some(path)) => format!("Saved to {}", path.display()),
                        Ok(None) => "Save cancelled.".into(),
                        Err(err) => format!("Save failed: {err}"),
                    };
                    for tab in &mut self.tabs {
                        if let AppTab::ImportExport { status, .. } = tab {
                            *status = msg.clone();
                        }
                    }
                }
            }
        }
    }

    fn apply_updated_vars(
        &mut self,
        globals: HashMap<String, String>,
        env_vars: HashMap<String, String>,
        (col_id, col_vars): (Option<Uuid>, HashMap<String, String>),
    ) {
        merge_var_map(&mut self.settings.global_variables, &globals);
        if let Some(eid) = self.settings.active_environment_id {
            if let Some(env) = self.environments.iter_mut().find(|e| e.id == eid) {
                merge_var_map(&mut env.variables, &env_vars);
                let _ = self.json_store.save_environment(env);
            }
        }
        if let Some(cid) = col_id {
            if let Some(col) = self.collections.iter_mut().find(|c| c.id == cid) {
                merge_var_map(&mut col.variables, &col_vars);
                let _ = self.json_store.save_collection(col);
            }
        }
    }

    pub fn send_active_http_request(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab).and_then(AppTab::as_request) else {
            return;
        };
        if !matches!(
            tab.request.protocol,
            ProtocolConfig::Http | ProtocolConfig::GraphQL { .. }
        ) {
            return;
        }
        let tab_id = tab.id;
        let req = tab.request.clone();
        let col_id = tab.collection_id;
        let parent_auth = col_id
            .and_then(|cid| self.collections.iter().find(|c| c.id == cid))
            .map(|c| c.auth.clone())
            .unwrap_or(AuthConfig::NoAuth);

        let vars = self.build_variable_context(col_id);
        let settings = self.settings.clone();
        let cookies = self.cookies.clone();
        let tx = self.tx.clone();

        let handle = self.rt.spawn(async move {
            let tx_panic = tx.clone();
            let inner = tokio::spawn(async move {
                let mut vars = vars;
                let mut extra_logs = Vec::new();
                let mut updated_globals = vars.global.clone();
                let mut updated_env = vars.environment.clone();
                let mut updated_col = vars.collection.clone();

                // 1. Pre-request script
                if !req.scripts.pre_request.trim().is_empty() {
                    let input = ScriptExecutionInput {
                        request: req.clone(),
                        response: None,
                        globals: updated_globals.clone(),
                        collection_vars: updated_col.clone(),
                        environment_vars: updated_env.clone(),
                        local_vars: vars.local.clone(),
                        data_vars: vars.data.clone(),
                    };
                    let out = run_script(&req.scripts.pre_request, input, 3_000);
                    extra_logs.extend(out.console_logs);
                    if let Some(err) = out.error {
                        extra_logs.push(ConsoleLogEntry {
                            level: ConsoleLogLevel::Error,
                            message: format!("Pre-request script error: {err}"),
                            timestamp: Utc::now(),
                        });
                    }
                    updated_globals = out.updated_globals;
                    updated_col = out.updated_collection_vars;
                    updated_env = out.updated_environment_vars;
                    vars.global = updated_globals.clone();
                    vars.collection = updated_col.clone();
                    vars.environment = updated_env.clone();
                    vars.local = out.updated_local_vars;
                }

                // 2. Resolve auth inheritance + variable interpolation + apply auth
                let resolver = VariableResolver::new(vars.clone());
                let effective_auth = resolve_auth(&[&req.auth, &parent_auth]);
                let resolved_auth = resolve_auth_variables(effective_auth, &resolver);
                let auth_ctx = build_auth_context(&req, &resolver);

                let auth_app = match apply_auth(&resolved_auth, &auth_ctx) {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = tx.send(UiMessage::HttpFinished {
                            tab_id,
                            result: Err(format!("Auth error: {e}")),
                            updated_globals,
                            updated_env,
                            updated_collection: (col_id, updated_col),
                            extra_logs,
                        });
                        return;
                    }
                };

                // 3. Execute HTTP request
                let opts = HttpRequestOptions {
                    extra_headers: auth_app.headers,
                    extra_query: auth_app.query_params,
                    cookies,
                    settings,
                };

                let mut http_res = HttpClient::new()
                    .execute(&req, &vars, &opts)
                    .await
                    .map_err(|e| e.to_string());

                // 4. Post-response test script
                if let Ok(resp) = &mut http_res {
                    if !req.scripts.post_response.trim().is_empty() {
                        let input = ScriptExecutionInput {
                            request: req.clone(),
                            response: Some(resp.clone()),
                            globals: updated_globals.clone(),
                            collection_vars: updated_col.clone(),
                            environment_vars: updated_env.clone(),
                            local_vars: vars.local.clone(),
                            data_vars: vars.data.clone(),
                        };
                        let out = run_script(&req.scripts.post_response, input, 3_000);
                        resp.test_results.extend(out.test_results);
                        extra_logs.extend(out.console_logs);
                        if let Some(err) = out.error {
                            extra_logs.push(ConsoleLogEntry {
                                level: ConsoleLogLevel::Error,
                                message: format!("Test script error: {err}"),
                                timestamp: Utc::now(),
                            });
                        }
                        updated_globals = out.updated_globals;
                        updated_col = out.updated_collection_vars;
                        updated_env = out.updated_environment_vars;
                    }
                }

                let _ = tx.send(UiMessage::HttpFinished {
                    tab_id,
                    result: http_res,
                    updated_globals,
                    updated_env,
                    updated_collection: (col_id, updated_col),
                    extra_logs,
                });
            });

            if let Err(join_err) = inner.await {
                if !join_err.is_cancelled() {
                    let _ = tx_panic.send(UiMessage::HttpFinished {
                        tab_id,
                        result: Err(format!("Request task failed: {join_err}")),
                        updated_globals: HashMap::new(),
                        updated_env: HashMap::new(),
                        updated_collection: (col_id, HashMap::new()),
                        extra_logs: Vec::new(),
                    });
                }
            }
        });

        if let Some(tab) = self.tabs.get_mut(self.active_tab).and_then(AppTab::as_request_mut) {
            tab.in_flight = true;
            tab.last_error = None;
            tab.cancel_handle = Some(handle.abort_handle());
        }
    }

    fn save_active_tab(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab).and_then(AppTab::as_request_mut) else {
            return;
        };
        tab.dirty = false;
        tab.title = tab.request.name.clone();
        let req = tab.request.clone();
        let col_id = tab.collection_id;

        if let Some(cid) = col_id {
            if let Some(col) = self.collections.iter_mut().find(|c| c.id == cid) {
                if !update_request_in_nodes(&mut col.items, &req) {
                    col.items.push(CollectionNode::Request(req));
                }
                col.updated_at = Utc::now();
                let _ = self.json_store.save_collection(col);
            }
        } else {
            if self.collections.is_empty() {
                self.collections.push(Collection::new("My Collection"));
            }
            let cid = self.collections[0].id;
            self.collections[0].items.push(CollectionNode::Request(req));
            let _ = self.json_store.save_collection(&self.collections[0]);
            if let Some(tab) = self.tabs.get_mut(self.active_tab).and_then(AppTab::as_request_mut) {
                tab.collection_id = Some(cid);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (send, new_tab, close_tab, save, focus_url, focus_search, toggle_sidebar, toggle_console, toggle_code) =
            ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::Enter),
                    i.consume_key(Modifiers::COMMAND, Key::N),
                    i.consume_key(Modifiers::COMMAND, Key::W),
                    i.consume_key(Modifiers::COMMAND, Key::S),
                    i.consume_key(Modifiers::COMMAND, Key::L),
                    i.consume_key(Modifiers::COMMAND, Key::K),
                    i.consume_key(Modifiers::COMMAND, Key::B),
                    i.consume_key(Modifiers::COMMAND, Key::J),
                    i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C),
                )
            });

        if send {
            self.send_active_http_request();
        }
        if new_tab {
            self.tabs.push(AppTab::Request(Box::new(WorkbenchTab::new_protocol(
                ProtocolConfig::Http,
            ))));
            self.active_tab = self.tabs.len() - 1;
        }
        if close_tab {
            self.close_tab(self.active_tab);
        }
        if save {
            self.save_active_tab();
        }
        if focus_url {
            self.focus_url_next = true;
        }
        if focus_search {
            self.focus_search_next = true;
        }
        if toggle_sidebar {
            self.sidebar_open = !self.sidebar_open;
        }
        if toggle_console {
            self.console_open = !self.console_open;
        }
        if toggle_code {
            self.code_drawer_open = !self.code_drawer_open;
        }
    }

    fn render_top_bar(&mut self, ctx: &egui::Context, pal: &Palette) {
        TopBottomPanel::top("top_bar")
            .exact_height(42.0)
            .frame(
                Frame::none()
                    .fill(pal.bg_sidebar)
                    .inner_margin(Margin::symmetric(12.0, 6.0))
                    .stroke(Stroke::new(1.0_f32, pal.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("APITester")
                            .strong()
                            .size(15.5)
                            .color(pal.accent_indicator),
                    );
                    ui.add_space(10.0);

                    let search_id = Id::new("global_search_input");
                    let search_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .id(search_id)
                            .hint_text("Search... (Ctrl+K)")
                            .desired_width(190.0),
                    );
                    if self.focus_search_next {
                        search_resp.request_focus();
                        self.focus_search_next = false;
                    }

                    ui.menu_button("+ New", |ui| {
                        let choices = [
                            ("HTTP Request", ProtocolConfig::Http),
                            ("GraphQL Query", ProtocolConfig::GraphQL { schema_url: None }),
                            ("WebSocket", ProtocolConfig::WebSocket { subprotocols: Vec::new() }),
                            (
                                "gRPC Call",
                                ProtocolConfig::Grpc {
                                    proto_path: None,
                                    service: String::new(),
                                    method: String::new(),
                                    use_reflection: false,
                                },
                            ),
                            (
                                "MQTT Client",
                                ProtocolConfig::Mqtt {
                                    topic: String::new(),
                                    qos: 0,
                                    client_id: "apitester".into(),
                                },
                            ),
                            (
                                "Socket.IO",
                                ProtocolConfig::SocketIo {
                                    namespace: "/".into(),
                                    event: String::new(),
                                    version: "4".into(),
                                },
                            ),
                            (
                                "MCP Client",
                                ProtocolConfig::Mcp {
                                    transport: "http".into(),
                                    command: None,
                                    args: Vec::new(),
                                },
                            ),
                            ("SSE Stream", ProtocolConfig::Sse { last_event_id: None }),
                        ];
                        for (label, proto) in choices {
                            if ui.button(label).clicked() {
                                self.tabs
                                    .push(AppTab::Request(Box::new(WorkbenchTab::new_protocol(proto))));
                                self.active_tab = self.tabs.len() - 1;
                                ui.close_menu();
                            }
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let code_btn = if self.code_drawer_open {
                            egui::Button::new(RichText::new("Code").strong().color(pal.text_primary))
                                .fill(pal.accent_tint)
                                .stroke(Stroke::new(1.0_f32, pal.accent_indicator))
                                .rounding(Rounding::same(6.0))
                        } else {
                            egui::Button::new(RichText::new("Code").color(pal.text_secondary))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .rounding(Rounding::same(6.0))
                        };
                        if ui.add(code_btn).on_hover_text("Toggle Code Drawer (Ctrl+Shift+C)").clicked() {
                            self.code_drawer_open = !self.code_drawer_open;
                        }

                        if self.proxy_server.is_some()
                            && ui
                                .button(RichText::new("Proxy ON").small().strong().color(pal.success))
                                .clicked()
                        {
                            self.open_or_focus_proxy_tab();
                        }

                        let active_env_name = self
                            .settings
                            .active_environment_id
                            .and_then(|id| self.environments.iter().find(|e| e.id == id))
                            .map(|e| e.name.as_str())
                            .unwrap_or("No Environment");

                        ComboBox::from_id_salt("active_env_combo")
                            .width(145.0)
                            .selected_text(active_env_name)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        self.settings.active_environment_id.is_none(),
                                        "No Environment",
                                    )
                                    .clicked()
                                {
                                    self.settings.active_environment_id = None;
                                    let _ = self.sqlite_store.save_settings(&self.settings);
                                }
                                for env in &self.environments {
                                    let selected =
                                        self.settings.active_environment_id == Some(env.id);
                                    if ui.selectable_label(selected, &env.name).clicked() {
                                        self.settings.active_environment_id = Some(env.id);
                                        let _ = self.sqlite_store.save_settings(&self.settings);
                                    }
                                }
                            });
                    });
                });
            });
    }

    fn render_activity_rail(&mut self, ctx: &egui::Context, pal: &Palette) {
        SidePanel::left("activity_rail")
            .exact_width(58.0)
            .resizable(false)
            .frame(
                Frame::none()
                    .fill(pal.bg_sidebar)
                    .inner_margin(Margin::symmetric(4.0, 8.0))
                    .stroke(Stroke::new(1.0_f32, pal.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let sidebar_items = [
                        (SidebarTab::Collections, "Col", "Collections"),
                        (SidebarTab::Environments, "Env", "Environments"),
                        (SidebarTab::History, "Hist", "History"),
                        (SidebarTab::Cookies, "Cook", "Cookies"),
                    ];
                    for (tab, short, tip) in sidebar_items {
                        let active = self.sidebar_open && self.sidebar_tab == tab;
                        if rail_button(ui, short, active, pal).on_hover_text(tip).clicked() {
                            if active {
                                self.sidebar_open = false;
                            } else {
                                self.sidebar_tab = tab;
                                self.sidebar_open = true;
                            }
                        }
                    }

                    ui.add_space(4.0);
                    subtle_divider(ui, pal);
                    ui.add_space(4.0);

                    let runner_active = matches!(self.tabs.get(self.active_tab), Some(AppTab::Runner));
                    if rail_button(ui, "Run", runner_active, pal)
                        .on_hover_text("Collection Runner & Load Tester")
                        .clicked()
                    {
                        self.open_or_focus_runner_tab();
                    }

                    let proxy_active =
                        matches!(self.tabs.get(self.active_tab), Some(AppTab::CaptureProxy));
                    if rail_button(ui, "Prx", proxy_active, pal)
                        .on_hover_text("Local Capture Proxy")
                        .clicked()
                    {
                        self.open_or_focus_proxy_tab();
                    }

                    let import_active = matches!(
                        self.tabs.get(self.active_tab),
                        Some(AppTab::ImportExport { .. })
                    );
                    if rail_button(ui, "Imp", import_active, pal)
                        .on_hover_text("Import / Export")
                        .clicked()
                    {
                        self.open_or_focus_import_tab();
                    }
                });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let theme_label = if pal.is_dark { "Lit" } else { "Drk" };
                    if rail_button(ui, theme_label, false, pal)
                        .on_hover_text("Toggle Dark / Light Theme")
                        .clicked()
                    {
                        self.toggle_theme();
                        apply_theme(ctx, self.settings.theme);
                        self.theme_applied = Some(self.settings.theme);
                    }

                    let settings_active =
                        matches!(self.tabs.get(self.active_tab), Some(AppTab::Settings));
                    if rail_button(ui, "Set", settings_active, pal)
                        .on_hover_text("Settings")
                        .clicked()
                    {
                        self.open_or_focus_settings_tab();
                    }
                });
            });
    }

    fn render_sidebar(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.sidebar_open {
            return;
        }
        SidePanel::left("sidebar")
            .default_width(260.0)
            .min_width(220.0)
            .frame(
                Frame::none()
                    .fill(pal.bg_sidebar)
                    .inner_margin(Margin::same(10.0))
                    .stroke(Stroke::new(1.0_f32, pal.border_subtle)),
            )
            .show(ctx, |ui| {
                let header = match self.sidebar_tab {
                    SidebarTab::Collections => "Collections",
                    SidebarTab::Environments => "Environments",
                    SidebarTab::History => "History",
                    SidebarTab::Cookies => "Cookies",
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(header).strong().size(14.5));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match self.sidebar_tab {
                            SidebarTab::Collections => {
                                if ui.small_button("+ New").clicked() {
                                    let col = Collection::new(format!(
                                        "Collection {}",
                                        self.collections.len() + 1
                                    ));
                                    let _ = self.json_store.save_collection(&col);
                                    self.collections.push(col);
                                }
                            }
                            SidebarTab::Environments => {
                                if ui.small_button("+ New").clicked() {
                                    let env = Environment::new(format!(
                                        "Environment {}",
                                        self.environments.len() + 1
                                    ));
                                    let _ = self.json_store.save_environment(&env);
                                    self.environments.push(env);
                                }
                            }
                            SidebarTab::History => {
                                if ui.small_button("Clear").clicked() {
                                    let _ = self.sqlite_store.clear_history();
                                    self.history.clear();
                                }
                            }
                            SidebarTab::Cookies => {
                                if ui.small_button("Clear").clicked() {
                                    let _ = self.sqlite_store.clear_cookies();
                                    self.cookies.clear();
                                }
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                subtle_divider(ui, pal);
                ui.add_space(6.0);

                match self.sidebar_tab {
                    SidebarTab::Collections => self.render_collections_sidebar(ui, pal),
                    SidebarTab::Environments => self.render_environments_sidebar(ui),
                    SidebarTab::History => self.render_history_sidebar(ui, pal),
                    SidebarTab::Cookies => self.render_cookies_sidebar(ui, pal),
                }
            });
    }

    fn render_collections_sidebar(&mut self, ui: &mut Ui, pal: &Palette) {
        let query = self.search_query.to_ascii_lowercase();
        let mut open_req: Option<(RequestItem, Option<Uuid>)> = None;
        let mut delete_col_id: Option<Uuid> = None;
        let mut edit_col_id: Option<Uuid> = None;
        let mut add_req_col: Option<Uuid> = None;
        let mut add_folder_col: Option<Uuid> = None;

        if self.collections.is_empty() {
            ui.add_space(16.0);
            ui.label(
                RichText::new("No collections yet. Click + New or Import a Postman v2.1 file.")
                    .small()
                    .color(pal.text_muted),
            );
            return;
        }

        ScrollArea::vertical().id_salt("collections_scroll").show(ui, |ui| {
            for col in &self.collections {
                ui.horizontal(|ui| {
                    let header_id = ui.make_persistent_id(col.id);
                    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        header_id,
                        true,
                    );
                    let is_open = state.is_open();
                    let arrow = if is_open { "v" } else { ">" };
                    if ghost_button(ui, arrow, pal).clicked() {
                        let mut s = state;
                        s.toggle(ui);
                        s.store(ui.ctx());
                    }
                    ui.label(RichText::new(&col.name).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("...", |ui| {
                            if ui.button("+ Add Request").clicked() {
                                add_req_col = Some(col.id);
                                ui.close_menu();
                            }
                            if ui.button("+ Add Folder").clicked() {
                                add_folder_col = Some(col.id);
                                ui.close_menu();
                            }
                            if ui.button("Auth & Variables").clicked() {
                                edit_col_id = Some(col.id);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui
                                .button(RichText::new("Delete Collection").color(pal.error))
                                .clicked()
                            {
                                delete_col_id = Some(col.id);
                                ui.close_menu();
                            }
                        });
                    });
                });

                let header_id = ui.make_persistent_id(col.id);
                let is_open = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    header_id,
                    true,
                )
                .is_open();
                if is_open {
                    ui.indent(col.id, |ui| {
                        render_collection_nodes(ui, &col.items, col.id, &query, pal, &mut open_req);
                    });
                }
                ui.add_space(4.0);
            }
        });

        if let Some((req, cid)) = open_req {
            self.open_or_focus_request(req, cid);
        }
        if let Some(cid) = edit_col_id {
            self.open_or_focus_collection_settings(cid);
        }
        if let Some(cid) = add_req_col {
            if let Some(col) = self.collections.iter_mut().find(|c| c.id == cid) {
                let req = RequestItem::new("New Request", HttpMethod::Get, "https://httpbin.org/get");
                col.items.push(CollectionNode::Request(req.clone()));
                let _ = self.json_store.save_collection(col);
                self.open_or_focus_request(req, Some(cid));
            }
        }
        if let Some(cid) = add_folder_col {
            if let Some(col) = self.collections.iter_mut().find(|c| c.id == cid) {
                col.items.push(CollectionNode::Folder(Folder::new("New Folder")));
                let _ = self.json_store.save_collection(col);
            }
        }
        if let Some(cid) = delete_col_id {
            self.collections.retain(|c| c.id != cid);
            let _ = self.json_store.delete_collection(cid);
        }
    }

    fn render_environments_sidebar(&mut self, ui: &mut Ui) {
        ui.checkbox(&mut self.reveal_secrets, "Show secrets");
        ui.add_space(4.0);

        ScrollArea::vertical().id_salt("envs_scroll").show(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Globals").strong())
                .default_open(false)
                .show(ui, |ui| {
                    if variables_table_ui(
                        ui,
                        "globals_vars",
                        &mut self.settings.global_variables,
                        self.reveal_secrets,
                    ) {
                        let _ = self.sqlite_store.save_settings(&self.settings);
                    }
                });

            ui.separator();
            let mut delete_env = None;
            let mut save_env_idx = None;
            for (idx, env) in self.environments.iter_mut().enumerate() {
                let is_active = self.settings.active_environment_id == Some(env.id);
                let label = if is_active {
                    format!("* {}", env.name)
                } else {
                    env.name.clone()
                };
                egui::CollapsingHeader::new(RichText::new(label).strong())
                    .id_salt(env.id)
                    .default_open(is_active)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("Set Active").clicked() {
                                self.settings.active_environment_id = Some(env.id);
                                let _ = self.sqlite_store.save_settings(&self.settings);
                            }
                            if ui.small_button("Delete").clicked() {
                                delete_env = Some(env.id);
                            }
                        });
                        if variables_table_ui(
                            ui,
                            &format!("env_vars_{}", env.id),
                            &mut env.variables,
                            self.reveal_secrets,
                        ) {
                            save_env_idx = Some(idx);
                        }
                    });
            }

            if let Some(idx) = save_env_idx {
                let _ = self.json_store.save_environment(&self.environments[idx]);
            }
            if let Some(eid) = delete_env {
                self.environments.retain(|e| e.id != eid);
                let _ = self.json_store.delete_environment(eid);
                if self.settings.active_environment_id == Some(eid) {
                    self.settings.active_environment_id = None;
                }
            }
        });
    }

    fn render_history_sidebar(&mut self, ui: &mut Ui, pal: &Palette) {
        let query = self.search_query.to_ascii_lowercase();
        let mut open_entry = None;
        ScrollArea::vertical().id_salt("history_scroll").show(ui, |ui| {
            for (idx, entry) in self.history.iter().enumerate() {
                if !query.is_empty() && !entry.url.to_ascii_lowercase().contains(&query) {
                    continue;
                }
                ui.horizontal(|ui| {
                    method_badge(ui, &entry.method, pal.is_dark);
                    if ui.selectable_label(false, &entry.url).clicked() {
                        open_entry = Some(idx);
                    }
                });
                ui.horizontal(|ui| {
                    if let Some(status) = entry.status {
                        ui.label(
                            RichText::new(format!("{status}"))
                                .small()
                                .strong()
                                .color(status_color_for_mode(status, pal.is_dark)),
                        );
                    }
                    ui.label(
                        RichText::new(entry.timestamp.format("%H:%M:%S").to_string())
                            .small()
                            .color(pal.text_muted),
                    );
                });
                subtle_divider(ui, pal);
                ui.add_space(4.0);
            }
        });

        if let Some(idx) = open_entry {
            let entry = self.history[idx].clone();
            let mut tab = WorkbenchTab::from_request(entry.request, None);
            tab.response = entry.response;
            self.tabs.push(AppTab::Request(Box::new(tab)));
            self.active_tab = self.tabs.len() - 1;
        }
    }

    fn render_cookies_sidebar(&mut self, ui: &mut Ui, pal: &Palette) {
        let mut del_cookie = None;
        ScrollArea::vertical().id_salt("cookies_scroll").show(ui, |ui| {
            for (idx, c) in self.cookies.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.strong(&c.domain);
                    ui.label(RichText::new(format!("{}={}", c.name, c.value)).color(pal.text_secondary));
                    if ui.small_button("x").clicked() {
                        del_cookie = Some(idx);
                    }
                });
            }
        });

        if let Some(idx) = del_cookie {
            let c = self.cookies.remove(idx);
            let _ = self.sqlite_store.delete_cookie(&c.domain, &c.path, &c.name);
        }
    }

    fn render_console_drawer(&mut self, ctx: &egui::Context, pal: &Palette) {
        TopBottomPanel::bottom("console_drawer")
            .frame(
                Frame::none()
                    .fill(pal.bg_sidebar)
                    .inner_margin(Margin::symmetric(12.0, 5.0))
                    .stroke(Stroke::new(1.0_f32, pal.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let arrow = if self.console_open { "v" } else { "^" };
                    if ghost_button(
                        ui,
                        &format!("[{arrow}] Wire Console ({})", self.console_logs.len()),
                        pal,
                    )
                    .on_hover_text("Toggle Wire Console (Ctrl+J)")
                    .clicked()
                    {
                        self.console_open = !self.console_open;
                    }
                    if self.console_open && ghost_button(ui, "Clear", pal).clicked() {
                        self.console_logs.clear();
                    }
                });

                if self.console_open {
                    subtle_divider(ui, pal);
                    ui.add_space(4.0);
                    ScrollArea::vertical()
                        .id_salt("wire_console_scroll")
                        .max_height(145.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for entry in &self.console_logs {
                                let color = match entry.level {
                                    ConsoleLogLevel::Error => pal.error,
                                    ConsoleLogLevel::Warn => pal.warning,
                                    ConsoleLogLevel::Info => pal.info,
                                    ConsoleLogLevel::Log => pal.text_secondary,
                                };
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new(entry.timestamp.format("%H:%M:%S").to_string())
                                            .small()
                                            .color(pal.text_muted),
                                    );
                                    ui.label(RichText::new(&entry.message).monospace().color(color));
                                });
                            }
                        });
                }
            });
    }

    fn render_workbench(&mut self, ctx: &egui::Context, pal: &Palette) {
        CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(pal.bg_canvas)
                    .inner_margin(Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                // Modern Workbench Tab Strip (single-line horizontal scroll)
                let mut close_idx = None;
                ScrollArea::horizontal()
                    .id_salt("tab_strip_scroll")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (idx, tab) in self.tabs.iter().enumerate() {
                        let selected = idx == self.active_tab;
                        let (badge, title) = match tab {
                            AppTab::Request(r) => {
                                let b = match &r.request.protocol {
                                    ProtocolConfig::Http => r.request.method.as_str(),
                                    ProtocolConfig::GraphQL { .. } => "GQL",
                                    ProtocolConfig::WebSocket { .. } => "WS",
                                    ProtocolConfig::Grpc { .. } => "gRPC",
                                    ProtocolConfig::Mqtt { .. } => "MQTT",
                                    ProtocolConfig::SocketIo { .. } => "SIO",
                                    ProtocolConfig::Mcp { .. } => "MCP",
                                    ProtocolConfig::Sse { .. } => "SSE",
                                };
                                let dirty = if r.dirty { " *" } else { "" };
                                (b.to_string(), format!("{}{dirty}", r.title))
                            }
                            AppTab::Runner => ("RUN".into(), "Runner & Load".into()),
                            AppTab::ImportExport { .. } => ("IMP".into(), "Import / Export".into()),
                            AppTab::CaptureProxy => ("PRX".into(), "Capture Proxy".into()),
                            AppTab::Settings => ("SET".into(), "Settings".into()),
                            AppTab::CollectionSettings { collection_id } => {
                                let cname = self
                                    .collections
                                    .iter()
                                    .find(|c| c.id == *collection_id)
                                    .map(|c| c.name.as_str())
                                    .unwrap_or("Collection");
                                ("COL".into(), cname.to_string())
                            }
                        };

                        let tab_bg = if selected {
                            pal.bg_surface
                        } else {
                            Color32::TRANSPARENT
                        };
                        let tab_stroke = if selected {
                            Stroke::new(1.0_f32, pal.border_subtle)
                        } else {
                            Stroke::NONE
                        };

                        let resp = Frame::none()
                            .fill(tab_bg)
                            .stroke(tab_stroke)
                            .rounding(Rounding::same(6.0))
                            .inner_margin(Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let badge_color = match tab {
                                        AppTab::Request(r) => {
                                            method_color_for_mode(&r.request.method, pal.is_dark)
                                        }
                                        _ => pal.accent_indicator,
                                    };
                                    let label_color = if selected {
                                        pal.text_primary
                                    } else {
                                        pal.text_secondary
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(badge)
                                                    .small()
                                                    .strong()
                                                    .color(badge_color),
                                            )
                                            .fill(Color32::TRANSPARENT)
                                            .stroke(Stroke::NONE)
                                            .min_size(Vec2::ZERO),
                                        )
                                        .clicked()
                                        || ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(title).color(label_color),
                                                )
                                                .fill(Color32::TRANSPARENT)
                                                .stroke(Stroke::NONE)
                                                .min_size(Vec2::ZERO),
                                            )
                                            .clicked()
                                    {
                                        self.active_tab = idx;
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("x").small().color(pal.text_muted),
                                            )
                                            .fill(Color32::TRANSPARENT)
                                            .stroke(Stroke::NONE)
                                            .min_size(Vec2::ZERO),
                                        )
                                        .clicked()
                                    {
                                        close_idx = Some(idx);
                                    }
                                });
                            })
                            .response;

                        if selected {
                            ui.painter().hline(
                                resp.rect.left() + 6.0..=resp.rect.right() - 6.0,
                                resp.rect.top() + 1.0,
                                Stroke::new(2.0_f32, pal.accent_indicator),
                            );
                        }
                    }

                    if ghost_button(ui, "+", pal)
                        .on_hover_text("New HTTP Tab (Ctrl+N)")
                        .clicked()
                    {
                        self.tabs.push(AppTab::Request(Box::new(WorkbenchTab::new_protocol(
                            ProtocolConfig::Http,
                        ))));
                        self.active_tab = self.tabs.len() - 1;
                    }
                        });
                    });

                if let Some(idx) = close_idx {
                    self.close_tab(idx);
                }

                ui.add_space(4.0);
                subtle_divider(ui, pal);
                ui.add_space(10.0);

                if self.tabs.is_empty() {
                    return;
                }

                match self.tabs.get(self.active_tab) {
                    Some(AppTab::Runner) => render_runner_tab(ui, self),
                    Some(AppTab::ImportExport { .. }) => render_import_export_tab(ui, ctx, self),
                    Some(AppTab::CaptureProxy) => render_capture_proxy_tab(ui, self),
                    Some(AppTab::Settings) => render_settings_tab(ui, ctx, self),
                    Some(AppTab::CollectionSettings { collection_id }) => {
                        let cid = *collection_id;
                        render_collection_settings_tab(ui, self, cid);
                    }
                    Some(AppTab::Request(req_tab)) => {
                        let col_id = req_tab.collection_id;
                        let is_http_like = matches!(
                            req_tab.request.protocol,
                            ProtocolConfig::Http | ProtocolConfig::GraphQL { .. }
                        );
                        let vars = self.build_variable_context(col_id);

                        if !is_http_like {
                            let rt = self.rt.clone();
                            let tx = self.tx.clone();
                            if let Some(tab) = self.tabs[self.active_tab].as_request_mut() {
                                protocol_workbench_ui(ui, tab, &rt, &tx, &vars);
                            }
                        } else {
                            self.render_http_workbench(ui, &vars, pal);
                        }
                    }
                    None => {}
                }
            });
    }

    fn render_http_workbench(&mut self, ui: &mut Ui, vars: &VariableContext, pal: &Palette) {
        let mut trigger_send = false;
        let mut trigger_save = false;
        let mut trigger_code = false;
        let mut trigger_introspect = false;
        let focus_url = std::mem::take(&mut self.focus_url_next);

        {
            let Some(tab) = self.tabs[self.active_tab].as_request_mut() else {
                return;
            };
            let resolver = VariableResolver::new(vars.clone());

            // Unified 34px URL Bar + Send primary CTA
            ui.horizontal(|ui| {
                let bar_width = (ui.available_width() - 205.0).max(160.0);
                Frame::none()
                    .fill(pal.bg_input)
                    .stroke(Stroke::new(1.0_f32, pal.border_subtle))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::symmetric(6.0, 3.0))
                    .show(ui, |ui| {
                        ui.set_min_height(28.0);
                        ui.horizontal(|ui| {
                            let current_method = tab.request.method.clone();
                            ComboBox::from_id_salt("http_method_combo")
                                .width(84.0)
                                .selected_text(
                                    RichText::new(current_method.as_str())
                                        .strong()
                                        .color(method_color_for_mode(&current_method, pal.is_dark)),
                                )
                                .show_ui(ui, |ui| {
                                    for m in [
                                        HttpMethod::Get,
                                        HttpMethod::Post,
                                        HttpMethod::Put,
                                        HttpMethod::Patch,
                                        HttpMethod::Delete,
                                        HttpMethod::Head,
                                        HttpMethod::Options,
                                    ] {
                                        let label = RichText::new(m.as_str())
                                            .strong()
                                            .color(method_color_for_mode(&m, pal.is_dark));
                                        if ui
                                            .selectable_value(&mut tab.request.method, m, label)
                                            .changed()
                                        {
                                            tab.dirty = true;
                                        }
                                    }
                                });

                            let url_id = Id::new("url_bar_input");
                            let url_resp = ui.add(
                                egui::TextEdit::singleline(&mut tab.request.url)
                                    .id(url_id)
                                    .frame(false)
                                    .font(egui::TextStyle::Monospace)
                                    .hint_text("https://api.example.com/v1/resource (Ctrl+L)")
                                    .desired_width((bar_width - 104.0).max(90.0)),
                            );
                            if focus_url {
                                url_resp.request_focus();
                            }
                            if url_resp.changed() {
                                tab.dirty = true;
                                sync_params_from_url(&tab.request.url, &mut tab.request.params);
                            }
                            if tab.request.url.contains("{{") {
                                url_resp.on_hover_text(format!(
                                    "Resolved: {}",
                                    resolver.resolve(&tab.request.url)
                                ));
                            }
                        });
                    });

                if tab.in_flight {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Cancel").strong().color(Color32::WHITE))
                                .fill(pal.error)
                                .stroke(Stroke::NONE)
                                .rounding(Rounding::same(6.0))
                                .min_size(Vec2::new(84.0, 34.0)),
                        )
                        .clicked()
                    {
                        if let Some(h) = tab.cancel_handle.take() {
                            h.abort();
                        }
                        tab.in_flight = false;
                    }
                } else if primary_button(ui, "Send", Vec2::new(84.0, 34.0))
                    .on_hover_text("Send request (Ctrl+Enter)")
                    .clicked()
                {
                    trigger_send = true;
                }

                if ghost_button(ui, "Save", pal)
                    .on_hover_text("Save request (Ctrl+S)")
                    .clicked()
                {
                    trigger_save = true;
                }
                if ghost_button(ui, "Code", pal)
                    .on_hover_text("Toggle Code Snippet drawer (Ctrl+Shift+C)")
                    .clicked()
                {
                    trigger_code = true;
                }
            });

            ui.add_space(8.0);

            // Underline Request Sub-tabs
            ui.horizontal(|ui| {
                for (sub, label) in [
                    (RequestSubTab::Params, format!("Params ({})", tab.request.params.len())),
                    (RequestSubTab::Auth, "Authorization".into()),
                    (RequestSubTab::Headers, format!("Headers ({})", tab.request.headers.len())),
                    (RequestSubTab::Body, "Body".into()),
                    (RequestSubTab::PreRequest, "Pre-request".into()),
                    (RequestSubTab::Tests, "Tests".into()),
                    (RequestSubTab::Settings, "Settings".into()),
                ] {
                    if underline_tab(ui, tab.req_sub_tab == sub, &label, pal).clicked() {
                        tab.req_sub_tab = sub;
                    }
                }
            });
            subtle_divider(ui, pal);
            ui.add_space(6.0);

            // Proportional Request Editor Pane
            let req_max_h = (ui.available_height() * 0.42).clamp(180.0, 420.0);
            ScrollArea::vertical()
                .id_salt("req_editor_scroll")
                .max_height(req_max_h)
                .show(ui, |ui| match tab.req_sub_tab {
                    RequestSubTab::Params => {
                        if kv_table_ui(ui, "req_params", &mut tab.request.params) {
                            tab.dirty = true;
                            sync_url_from_params(&mut tab.request.url, &tab.request.params);
                        }
                    }
                    RequestSubTab::Auth => {
                        if auth_editor_ui(ui, "req_auth", &mut tab.request.auth) {
                            tab.dirty = true;
                        }
                    }
                    RequestSubTab::Headers => {
                        if kv_table_ui(ui, "req_headers", &mut tab.request.headers) {
                            tab.dirty = true;
                        }
                    }
                    RequestSubTab::Body => {
                        render_body_editor(ui, tab, pal, &mut trigger_introspect);
                    }
                    RequestSubTab::PreRequest => {
                        ui.label(
                            RichText::new("JavaScript executed before the request is sent (pm.* API):")
                                .small()
                                .color(pal.text_muted),
                        );
                        if ui
                            .add(
                                egui::TextEdit::multiline(&mut tab.request.scripts.pre_request)
                                    .desired_rows(8)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            )
                            .changed()
                        {
                            tab.dirty = true;
                        }
                    }
                    RequestSubTab::Tests => {
                        ui.label(
                            RichText::new("JavaScript executed after the response arrives (pm.test / pm.expect):")
                                .small()
                                .color(pal.text_muted),
                        );
                        if ui
                            .add(
                                egui::TextEdit::multiline(&mut tab.request.scripts.post_response)
                                    .desired_rows(8)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            )
                            .changed()
                        {
                            tab.dirty = true;
                        }
                    }
                    RequestSubTab::Settings => {
                        ui.horizontal(|ui| {
                            ui.label("Request Name:");
                            if ui
                                .add(egui::TextEdit::singleline(&mut tab.request.name).desired_width(280.0))
                                .changed()
                            {
                                tab.dirty = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Description:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut tab.request.description)
                                        .desired_width(360.0),
                                )
                                .changed()
                            {
                                tab.dirty = true;
                            }
                        });
                    }
                });

            ui.add_space(6.0);
            subtle_divider(ui, pal);
            ui.add_space(6.0);

            // Response Pane
            if let Some(err) = &tab.last_error {
                Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(pal.error.r(), pal.error.g(), pal.error.b(), 24))
                    .stroke(Stroke::new(1.0_f32, pal.error))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::symmetric(10.0, 6.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("Error: {err}")).color(pal.error));
                    });
                ui.add_space(6.0);
            }

            if let Some(resp) = tab.response.clone() {
                ui.horizontal_wrapped(|ui| {
                    status_badge(ui, resp.status, &resp.status_text, pal.is_dark);
                    ui.label(
                        RichText::new(format!("{:.1} ms", resp.timings.total_ms))
                            .small()
                            .color(pal.text_secondary),
                    )
                    .on_hover_text(format!(
                        "DNS: {:.1} ms\nTCP: {:.1} ms\nTLS: {:.1} ms\nTTFB: {:.1} ms\nTotal: {:.1} ms",
                        resp.timings.dns_ms,
                        resp.timings.tcp_ms,
                        resp.timings.tls_ms,
                        resp.timings.ttfb_ms,
                        resp.timings.total_ms
                    ));
                    ui.label(
                        RichText::new(format_bytes(resp.size_bytes))
                            .small()
                            .color(pal.text_secondary),
                    );

                    ui.add_space(8.0);
                    let passed = resp.test_results.iter().filter(|t| t.passed).count();
                    for (sub, label) in [
                        (ResponseSubTab::Body, "Body".into()),
                        (ResponseSubTab::Headers, format!("Headers ({})", resp.headers.len())),
                        (ResponseSubTab::Cookies, format!("Cookies ({})", resp.cookies.len())),
                        (
                            ResponseSubTab::TestResults,
                            format!("Tests ({passed}/{})", resp.test_results.len()),
                        ),
                        (ResponseSubTab::Console, format!("Console ({})", resp.console_logs.len())),
                    ] {
                        if underline_tab(ui, tab.resp_sub_tab == sub, &label, pal).clicked() {
                            tab.resp_sub_tab = sub;
                        }
                    }

                    ui.add_space(8.0);
                    if ghost_button(ui, "Save Example", pal).clicked() {
                        tab.request.examples.push(SavedExample {
                            id: Uuid::new_v4(),
                            name: format!("Example ({})", resp.status),
                            request_url: tab.request.url.clone(),
                            request_method: tab.request.method.clone(),
                            request_headers: tab.request.headers.clone(),
                            request_body: tab.request.body.clone(),
                            response: resp.clone(),
                        });
                        tab.dirty = true;
                    }
                });
                subtle_divider(ui, pal);
                ui.add_space(6.0);

                match tab.resp_sub_tab {
                    ResponseSubTab::Body => {
                        let raw_text = resp.body_text().into_owned();
                        let display_text = match tab.resp_body_view {
                            ResponseBodyView::Pretty => pretty_format_body(&raw_text),
                            ResponseBodyView::Raw => raw_text,
                        };
                        ui.horizontal(|ui| {
                            if underline_tab(
                                ui,
                                tab.resp_body_view == ResponseBodyView::Pretty,
                                "Pretty",
                                pal,
                            )
                            .clicked()
                            {
                                tab.resp_body_view = ResponseBodyView::Pretty;
                            }
                            if underline_tab(
                                ui,
                                tab.resp_body_view == ResponseBodyView::Raw,
                                "Raw",
                                pal,
                            )
                            .clicked()
                            {
                                tab.resp_body_view = ResponseBodyView::Raw;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut tab.resp_search)
                                    .hint_text("Filter response...")
                                    .desired_width(180.0),
                            );
                            if ghost_button(ui, "Copy", pal).clicked() {
                                ui.output_mut(|o| o.copied_text = display_text.clone());
                            }
                        });
                        ui.add_space(4.0);
                        Frame::none()
                            .fill(pal.bg_input)
                            .stroke(Stroke::new(1.0_f32, pal.border_subtle))
                            .rounding(Rounding::same(6.0))
                            .inner_margin(Margin::same(10.0))
                            .show(ui, |ui| {
                                ScrollArea::both().id_salt("resp_body_scroll").show(ui, |ui| {
                                    let job =
                                        highlight_body(&display_text, pal.is_dark, &tab.resp_search);
                                    ui.label(job);
                                });
                            });
                    }
                    ResponseSubTab::Headers => {
                        ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("resp_headers_grid")
                                .num_columns(2)
                                .striped(true)
                                .spacing([12.0, 6.0])
                                .show(ui, |ui| {
                                    ui.strong("Header");
                                    ui.strong("Value");
                                    ui.end_row();
                                    for h in &resp.headers {
                                        ui.monospace(&h.key);
                                        ui.monospace(&h.value);
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                    ResponseSubTab::Cookies => {
                        ScrollArea::vertical().show(ui, |ui| {
                            for c in &resp.cookies {
                                ui.horizontal(|ui| {
                                    ui.strong(&c.name);
                                    ui.monospace(&c.value);
                                    ui.label(
                                        RichText::new(format!("{} {}", c.domain, c.path))
                                            .small()
                                            .color(pal.text_muted),
                                    );
                                });
                            }
                        });
                    }
                    ResponseSubTab::TestResults => {
                        ScrollArea::vertical().show(ui, |ui| {
                            for t in &resp.test_results {
                                let (badge, color) = if t.passed {
                                    ("PASS", pal.success)
                                } else {
                                    ("FAIL", pal.error)
                                };
                                ui.horizontal(|ui| {
                                    Frame::none()
                                        .fill(Color32::from_rgba_unmultiplied(
                                            color.r(),
                                            color.g(),
                                            color.b(),
                                            32,
                                        ))
                                        .rounding(Rounding::same(4.0))
                                        .inner_margin(Margin::symmetric(6.0, 1.5))
                                        .show(ui, |ui| {
                                            ui.label(RichText::new(badge).small().strong().color(color));
                                        });
                                    ui.label(&t.name);
                                    if let Some(err) = &t.error {
                                        ui.colored_label(color, err);
                                    }
                                });
                            }
                        });
                    }
                    ResponseSubTab::Console => {
                        ScrollArea::vertical().show(ui, |ui| {
                            for entry in &resp.console_logs {
                                ui.monospace(format!(
                                    "[{}] {}",
                                    entry.timestamp.format("%H:%M:%S"),
                                    entry.message
                                ));
                            }
                        });
                    }
                }
            } else if tab.in_flight {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(
                            RichText::new("Press Ctrl+Enter or click Send to execute")
                                .size(14.5)
                                .color(pal.text_secondary),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Ctrl+L Focus URL   |   Ctrl+N New Tab   |   Ctrl+Shift+C Code Snippet")
                                .small()
                                .color(pal.text_muted),
                        );
                    });
                });
            }
        }

        if trigger_send {
            self.send_active_http_request();
        }
        if trigger_save {
            self.save_active_tab();
        }
        if trigger_code {
            self.code_drawer_open = !self.code_drawer_open;
        }
        if trigger_introspect {
            if let Some(tab) = self.tabs[self.active_tab].as_request() {
                let tab_id = tab.id;
                let url = tab.request.url.clone();
                let headers = tab.request.headers.clone();
                let vars_clone = vars.clone();
                let tx = self.tx.clone();
                self.rt.spawn(async move {
                    let res = introspect_schema(
                        &HttpClient::new(),
                        &url,
                        &headers,
                        &vars_clone,
                        &HttpRequestOptions::default(),
                    )
                    .await
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                    .map_err(|e| e.to_string());
                    let _ = tx.send(UiMessage::GraphQlIntrospected { tab_id, result: res });
                });
            }
        }
    }
}

impl eframe::App for ApiTesterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ScreenshotTour::tick(self, ctx);

        if self.theme_applied != Some(self.settings.theme) {
            apply_theme(ctx, self.settings.theme);
            self.theme_applied = Some(self.settings.theme);
        }

        self.handle_shortcuts(ctx);
        self.drain_messages();

        let pal = Palette::for_mode(!matches!(self.settings.theme, ThemeMode::Light));
        self.render_top_bar(ctx, &pal);
        self.render_activity_rail(ctx, &pal);
        self.render_sidebar(ctx, &pal);
        self.render_console_drawer(ctx, &pal);
        render_code_drawer(ctx, self);
        self.render_workbench(ctx, &pal);

        if self.tabs.iter().filter_map(AppTab::as_request).any(|t| {
            t.in_flight || t.ws_connected || t.mqtt_connected || t.sio_connected || t.sse_connected
        }) || self.proxy_server.is_some()
            || self.runner_in_flight
            || self.load_in_flight
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn rail_button(ui: &mut Ui, label: &str, active: bool, pal: &Palette) -> egui::Response {
    let fill = if active {
        pal.accent_tint
    } else {
        Color32::TRANSPARENT
    };
    let color = if active {
        pal.accent_indicator
    } else {
        pal.text_secondary
    };
    let resp = ui.add(
        egui::Button::new(RichText::new(label).small().strong().color(color))
            .fill(fill)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(6.0))
            .min_size(Vec2::new(46.0, 32.0)),
    );
    if active {
        ui.painter().vline(
            resp.rect.left() - 2.0,
            resp.rect.top() + 5.0..=resp.rect.bottom() - 5.0,
            Stroke::new(2.0_f32, pal.accent_indicator),
        );
    }
    resp
}

fn render_body_editor(
    ui: &mut Ui,
    tab: &mut WorkbenchTab,
    pal: &Palette,
    trigger_introspect: &mut bool,
) {
    let mode_idx = match tab.request.body {
        RequestBody::None => 0,
        RequestBody::Raw { .. } => 1,
        RequestBody::UrlEncoded(_) => 2,
        RequestBody::FormData(_) => 3,
        RequestBody::Binary { .. } => 4,
        RequestBody::GraphQL { .. } => 5,
    };
    let mut selected = mode_idx;

    ui.horizontal_wrapped(|ui| {
        for (idx, label) in [
            "None",
            "Raw",
            "x-www-form-urlencoded",
            "form-data",
            "Binary",
            "GraphQL",
        ]
        .iter()
        .enumerate()
        {
            if underline_tab(ui, selected == idx, label, pal).clicked() {
                selected = idx;
            }
        }
    });

    if selected != mode_idx {
        tab.request.body = match selected {
            0 => RequestBody::None,
            1 => RequestBody::Raw {
                content: String::new(),
                language: RawLanguage::Json,
            },
            2 => RequestBody::UrlEncoded(Vec::new()),
            3 => RequestBody::FormData(Vec::new()),
            4 => RequestBody::Binary {
                path: String::new(),
            },
            _ => RequestBody::GraphQL {
                query: String::new(),
                variables: "{}".into(),
            },
        };
        tab.dirty = true;
    }

    ui.add_space(6.0);
    match &mut tab.request.body {
        RequestBody::None => {
            ui.label(
                RichText::new("This request does not have a body.")
                    .small()
                    .color(pal.text_muted),
            );
        }
        RequestBody::Raw { content, language } => {
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("raw_lang_combo")
                    .selected_text(format!("{language:?}"))
                    .show_ui(ui, |ui| {
                        for lang in [
                            RawLanguage::Json,
                            RawLanguage::Xml,
                            RawLanguage::Html,
                            RawLanguage::Javascript,
                            RawLanguage::Text,
                        ] {
                            ui.selectable_value(language, lang, format!("{lang:?}"));
                        }
                    });
                if *language == RawLanguage::Json && ui.small_button("Pretty Format").clicked() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
                        if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                            *content = pretty;
                            tab.dirty = true;
                        }
                    }
                }
            });
            if ui
                .add(
                    egui::TextEdit::multiline(content)
                        .desired_rows(7)
                        .desired_width(f32::INFINITY)
                        .code_editor(),
                )
                .changed()
            {
                tab.dirty = true;
            }
        }
        RequestBody::UrlEncoded(fields) => {
            if kv_table_ui(ui, "body_urlencoded", fields) {
                tab.dirty = true;
            }
        }
        RequestBody::FormData(fields) => {
            if form_data_table_ui(ui, "body_formdata", fields) {
                tab.dirty = true;
            }
        }
        RequestBody::Binary { path } => {
            ui.horizontal(|ui| {
                ui.label("File Path:");
                if ui
                    .add(
                        egui::TextEdit::singleline(path)
                            .hint_text("/path/to/payload.bin")
                            .desired_width(360.0),
                    )
                    .changed()
                {
                    tab.dirty = true;
                }
            });
        }
        RequestBody::GraphQL { query, variables } => {
            ui.horizontal(|ui| {
                ui.label("Query & Variables");
                if ui.small_button("Introspect Schema").clicked() {
                    *trigger_introspect = true;
                }
            });
            ui.columns(2, |cols| {
                if cols[0]
                    .add(
                        egui::TextEdit::multiline(query)
                            .desired_rows(7)
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .hint_text("query { ... }"),
                    )
                    .changed()
                {
                    tab.dirty = true;
                }
                if cols[1]
                    .add(
                        egui::TextEdit::multiline(variables)
                            .desired_rows(7)
                            .desired_width(f32::INFINITY)
                            .code_editor()
                            .hint_text("{\"id\": 1}"),
                    )
                    .changed()
                {
                    tab.dirty = true;
                }
            });
            if let Some(schema) = &tab.graphql_schema_json {
                ui.collapsing("Introspected Schema", |ui| {
                    ui.monospace(schema);
                });
            }
        }
    }
}

fn render_collection_nodes(
    ui: &mut Ui,
    nodes: &[CollectionNode],
    col_id: Uuid,
    query: &str,
    pal: &Palette,
    open_req: &mut Option<(RequestItem, Option<Uuid>)>,
) {
    for node in nodes {
        match node {
            CollectionNode::Folder(folder) => {
                egui::CollapsingHeader::new(RichText::new(&folder.name).color(pal.text_secondary))
                    .id_salt(folder.id)
                    .default_open(true)
                    .show(ui, |ui| {
                        render_collection_nodes(ui, &folder.items, col_id, query, pal, open_req);
                    });
            }
            CollectionNode::Request(req) => {
                if !query.is_empty()
                    && !req.name.to_ascii_lowercase().contains(query)
                    && !req.url.to_ascii_lowercase().contains(query)
                {
                    continue;
                }
                ui.horizontal(|ui| {
                    method_badge(ui, &req.method, pal.is_dark);
                    if ui
                        .add(
                            egui::Button::new(RichText::new(&req.name).color(pal.text_primary))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .rounding(Rounding::same(4.0)),
                        )
                        .clicked()
                    {
                        *open_req = Some((req.clone(), Some(col_id)));
                    }
                });
            }
        }
    }
}

fn update_request_in_nodes(nodes: &mut [CollectionNode], req: &RequestItem) -> bool {
    for node in nodes {
        match node {
            CollectionNode::Request(existing) if existing.id == req.id => {
                *existing = req.clone();
                return true;
            }
            CollectionNode::Folder(folder) => {
                if update_request_in_nodes(&mut folder.items, req) {
                    return true;
                }
            }
            CollectionNode::Request(_) => {}
        }
    }
    false
}

fn merge_var_map(target: &mut Vec<Variable>, updates: &HashMap<String, String>) {
    for (k, v) in updates {
        if let Some(existing) = target.iter_mut().find(|var| var.key == *k) {
            existing.value = v.clone();
        } else {
            target.push(Variable::new(k, v));
        }
    }
}

/// Syncs `RequestItem::params` from the query portion of `url`.
pub fn sync_params_from_url(url: &str, params: &mut Vec<KeyValue>) {
    let Some((_, query)) = url.split_once('?') else {
        return;
    };
    let parsed: Vec<KeyValue> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => KeyValue::new(k, v),
            None => KeyValue::new(pair, ""),
        })
        .collect();
    if !parsed.is_empty() {
        *params = parsed;
    }
}

/// Syncs the query string of `url` from enabled entries in `params`.
pub fn sync_url_from_params(url: &mut String, params: &[KeyValue]) {
    let base = url.split('?').next().unwrap_or("").to_string();
    let query: Vec<String> = params
        .iter()
        .filter(|kv| kv.enabled && !kv.key.is_empty())
        .map(|kv| format!("{}={}", kv.key, kv.value))
        .collect();
    *url = if query.is_empty() {
        base
    } else {
        format!("{base}?{}", query.join("&"))
    };
}

fn pretty_format_body(raw: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Ok(pretty) = serde_json::to_string_pretty(&val) {
            return pretty;
        }
    }
    raw.to_string()
}

fn build_auth_context(req: &RequestItem, resolver: &VariableResolver) -> AuthRequestContext {
    let mut url = resolver.resolve(&req.url);
    let enabled_params: Vec<String> = req
        .params
        .iter()
        .filter(|kv| kv.enabled && !kv.key.is_empty())
        .map(|kv| {
            format!(
                "{}={}",
                resolver.resolve(&kv.key),
                resolver.resolve(&kv.value)
            )
        })
        .collect();
    if !enabled_params.is_empty() && !url.contains('?') {
        url.push('?');
        url.push_str(&enabled_params.join("&"));
    } else if !enabled_params.is_empty() {
        let existing_query = url
            .split_once('?')
            .map(|(_, q)| q.to_string())
            .unwrap_or_default();
        for p in enabled_params {
            if !existing_query.split('&').any(|part| part == p) {
                url.push('&');
                url.push_str(&p);
            }
        }
    }

    let body = match &req.body {
        RequestBody::Raw { content, .. } => resolver.resolve(content).into_bytes(),
        RequestBody::UrlEncoded(fields) => fields
            .iter()
            .filter(|kv| kv.enabled && !kv.key.is_empty())
            .map(|kv| format!("{}={}", resolver.resolve(&kv.key), resolver.resolve(&kv.value)))
            .collect::<Vec<_>>()
            .join("&")
            .into_bytes(),
        RequestBody::GraphQL { query, variables } => {
            let vars_json: serde_json::Value = serde_json::from_str(&resolver.resolve(variables))
                .unwrap_or_else(|_| serde_json::json!({}));
            serde_json::json!({
                "query": resolver.resolve(query),
                "variables": vars_json,
            })
            .to_string()
            .into_bytes()
        }
        _ => Vec::new(),
    };

    AuthRequestContext {
        method: req.method.as_str().to_string(),
        url,
        headers: req
            .headers
            .iter()
            .filter(|h| h.enabled)
            .map(|h| (resolver.resolve(&h.key), resolver.resolve(&h.value)))
            .collect(),
        body,
    }
}

fn resolve_auth_variables(auth: &AuthConfig, resolver: &VariableResolver) -> AuthConfig {
    let r = |s: &str| resolver.resolve(s);
    let ro = |o: &Option<String>| o.as_deref().map(|s| resolver.resolve(s));
    match auth {
        AuthConfig::Inherit => AuthConfig::Inherit,
        AuthConfig::NoAuth => AuthConfig::NoAuth,
        AuthConfig::Bearer { token } => AuthConfig::Bearer { token: r(token) },
        AuthConfig::Basic { username, password } => AuthConfig::Basic {
            username: r(username),
            password: r(password),
        },
        AuthConfig::ApiKey { key, value, add_to } => AuthConfig::ApiKey {
            key: r(key),
            value: r(value),
            add_to: *add_to,
        },
        AuthConfig::OAuth2(c) => {
            let mut out = c.clone();
            out.access_token = r(&c.access_token);
            out.refresh_token = ro(&c.refresh_token);
            out.token_type = r(&c.token_type);
            out.header_prefix = r(&c.header_prefix);
            out.auth_url = r(&c.auth_url);
            out.token_url = r(&c.token_url);
            out.redirect_uri = r(&c.redirect_uri);
            out.client_id = r(&c.client_id);
            out.client_secret = r(&c.client_secret);
            out.scope = r(&c.scope);
            out.state = r(&c.state);
            AuthConfig::OAuth2(out)
        }
        AuthConfig::AwsV4(c) => {
            let mut out = c.clone();
            out.access_key = r(&c.access_key);
            out.secret_key = r(&c.secret_key);
            out.region = r(&c.region);
            out.service = r(&c.service);
            out.session_token = ro(&c.session_token);
            AuthConfig::AwsV4(out)
        }
        AuthConfig::Jwt(c) => {
            let mut out = c.clone();
            out.algorithm = r(&c.algorithm);
            out.secret = r(&c.secret);
            out.payload = r(&c.payload);
            out.header_prefix = r(&c.header_prefix);
            out.query_param_key = r(&c.query_param_key);
            out.custom_headers = ro(&c.custom_headers);
            AuthConfig::Jwt(out)
        }
        AuthConfig::Digest(c) => {
            let mut out = c.clone();
            out.username = r(&c.username);
            out.password = r(&c.password);
            out.realm = r(&c.realm);
            out.nonce = r(&c.nonce);
            out.algorithm = r(&c.algorithm);
            out.qop = r(&c.qop);
            out.nonce_count = r(&c.nonce_count);
            out.client_nonce = r(&c.client_nonce);
            out.opaque = r(&c.opaque);
            AuthConfig::Digest(out)
        }
        AuthConfig::OAuth1(c) => {
            let mut out = c.clone();
            out.consumer_key = r(&c.consumer_key);
            out.consumer_secret = r(&c.consumer_secret);
            out.token = r(&c.token);
            out.token_secret = r(&c.token_secret);
            out.signature_method = r(&c.signature_method);
            out.timestamp = ro(&c.timestamp);
            out.nonce = ro(&c.nonce);
            out.version = r(&c.version);
            out.realm = ro(&c.realm);
            AuthConfig::OAuth1(out)
        }
        AuthConfig::Ntlm(c) => {
            let mut out = c.clone();
            out.username = r(&c.username);
            out.password = r(&c.password);
            out.domain = r(&c.domain);
            out.workstation = r(&c.workstation);
            AuthConfig::Ntlm(out)
        }
        AuthConfig::Hawk(c) => {
            let mut out = c.clone();
            out.auth_id = r(&c.auth_id);
            out.auth_key = r(&c.auth_key);
            out.algorithm = r(&c.algorithm);
            out.user = r(&c.user);
            out.nonce = r(&c.nonce);
            out.extra_data = r(&c.extra_data);
            out.app = r(&c.app);
            out.delegation = r(&c.delegation);
            out.timestamp = ro(&c.timestamp);
            AuthConfig::Hawk(out)
        }
        AuthConfig::EdgeGrid(c) => {
            let mut out = c.clone();
            out.access_token = r(&c.access_token);
            out.client_token = r(&c.client_token);
            out.client_secret = r(&c.client_secret);
            out.nonce = r(&c.nonce);
            out.timestamp = ro(&c.timestamp);
            out.base_url = r(&c.base_url);
            out.headers_to_sign = r(&c.headers_to_sign);
            AuthConfig::EdgeGrid(out)
        }
        AuthConfig::Asap(c) => {
            let mut out = c.clone();
            out.alg = r(&c.alg);
            out.issuer = r(&c.issuer);
            out.kid = r(&c.kid);
            out.audience = r(&c.audience);
            out.subject = r(&c.subject);
            out.private_key = r(&c.private_key);
            out.claims = r(&c.claims);
            AuthConfig::Asap(out)
        }
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use apitester_import_export::generate_code;
    use tempfile::tempdir;

    use super::*;

    fn test_app(demo: bool) -> (ApiTesterApp, tokio::runtime::Runtime, tempfile::TempDir) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempdir().unwrap();
        let app = ApiTesterApp::new_headless(
            rt.handle().clone(),
            LaunchConfig {
                demo_mode: demo,
                data_dir: Some(dir.path().to_path_buf()),
                screenshot_dir: None,
            },
        );
        (app, rt, dir)
    }

    #[test]
    fn demo_mode_populates_collections_envs_tabs_runner_and_proxy() {
        let (app, _rt, _dir) = test_app(true);
        assert!(!app.collections.is_empty());
        assert_eq!(app.environments.len(), 2);
        assert_eq!(app.tabs.len(), 3);
        assert!(app.tabs[0].as_request().unwrap().response.is_some());
        assert!(!app.history.is_empty());
        assert!(app.runner_report.is_some());
        assert!(app.load_summary.is_some());
        assert!(!app.captured_exchanges.is_empty());
    }

    #[test]
    fn tab_open_focus_and_close_transitions() {
        let (mut app, _rt, _dir) = test_app(false);
        assert_eq!(app.tabs.len(), 1);

        let req = RequestItem::new("Users", HttpMethod::Get, "https://example.com/users");
        app.open_or_focus_request(req.clone(), None);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);

        app.open_or_focus_request(req, None);
        assert_eq!(app.tabs.len(), 2);

        // Singleton tool tabs focus instead of duplicating
        app.open_or_focus_runner_tab();
        assert_eq!(app.tabs.len(), 3);
        app.open_or_focus_runner_tab();
        assert_eq!(app.tabs.len(), 3);

        app.close_tab(2);
        app.close_tab(1);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, 0);

        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn url_and_params_sync_bidirectionally() {
        let mut params = Vec::new();
        sync_params_from_url("https://api.example.com/items?page=2&sort=asc", &mut params);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].key, "page");
        assert_eq!(params[0].value, "2");

        params[1].enabled = false;
        let mut url = "https://api.example.com/items?page=2&sort=asc".to_string();
        sync_url_from_params(&mut url, &params);
        assert_eq!(url, "https://api.example.com/items?page=2");
    }

    #[test]
    fn theme_toggle_and_variable_context_resolution() {
        let (mut app, _rt, _dir) = test_app(true);
        assert_eq!(app.settings.theme, ThemeMode::Dark);
        app.toggle_theme();
        assert_eq!(app.settings.theme, ThemeMode::Light);

        let cid = app.collections[0].id;
        let ctx = app.build_variable_context(Some(cid));
        assert_eq!(
            ctx.get("baseUrl"),
            Some("https://api.dev.apitester.local/v1")
        );
        assert_eq!(ctx.get("version"), Some("v1"));
    }

    #[test]
    fn codegen_integration_generates_all_6_languages() {
        let req = RequestItem::new("Get", HttpMethod::Get, "{{base}}/ping");
        let mut vars = VariableContext::new();
        vars.set(VariableScope::Environment, "base", "https://api.test");

        for lang in [
            CodegenLanguage::Curl,
            CodegenLanguage::RustReqwest,
            CodegenLanguage::PythonRequests,
            CodegenLanguage::JavaScriptFetch,
            CodegenLanguage::GoNetHttp,
            CodegenLanguage::PhpCurl,
        ] {
            let snippet = generate_code(&req, lang, &vars);
            assert!(
                snippet.contains("https://api.test/ping"),
                "missing URL in {lang:?}: {snippet}"
            );
        }
    }

    #[test]
    fn auth_variables_and_query_params_resolved_before_apply_auth() {
        let mut vars = VariableContext::new();
        vars.set(VariableScope::Environment, "baseUrl", "https://api.test");
        vars.set(VariableScope::Environment, "authToken", "secret_tok_42");
        let resolver = VariableResolver::new(vars);

        let mut req = RequestItem::new("Users", HttpMethod::Get, "{{baseUrl}}/users");
        req.params.push(KeyValue::new("limit", "10"));
        let auth = AuthConfig::Bearer {
            token: "{{authToken}}".into(),
        };

        let resolved_auth = resolve_auth_variables(&auth, &resolver);
        let auth_ctx = build_auth_context(&req, &resolver);
        assert_eq!(auth_ctx.url, "https://api.test/users?limit=10");

        let applied = apply_auth(&resolved_auth, &auth_ctx).unwrap();
        assert_eq!(
            applied.headers,
            vec![("Authorization".to_string(), "Bearer secret_tok_42".to_string())]
        );
    }

    #[test]
    fn send_active_http_request_completes_on_live_server_and_unresolved_url() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempdir().unwrap();

        let addr = rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                if let Ok((mut sock, _)) = listener.accept().await {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    let body = r#"{"ok":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            });
            addr
        });

        let mut app = ApiTesterApp::new_headless(
            rt.handle().clone(),
            LaunchConfig {
                demo_mode: false,
                data_dir: Some(dir.path().to_path_buf()),
                screenshot_dir: None,
            },
        );

        // 1. Live HTTP 200 test
        app.tabs[0].as_request_mut().unwrap().request.url = format!("http://{addr}/users/me");
        app.send_active_http_request();
        assert!(app.tabs[0].as_request().unwrap().in_flight);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while app.tabs[0].as_request().unwrap().in_flight && std::time::Instant::now() < deadline {
            app.drain_messages();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let tab = app.tabs[0].as_request().unwrap();
        assert!(!tab.in_flight, "request stayed in_flight");
        let resp = tab.response.as_ref().expect("expected response");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body_text(), r#"{"ok":true}"#);
        assert!(!app.console_logs.is_empty());

        // 2. Unresolved variable URL must fail cleanly and clear in_flight
        app.tabs[0].as_request_mut().unwrap().request.url = "{{url}}/users/me".to_string();
        app.send_active_http_request();
        assert!(app.tabs[0].as_request().unwrap().in_flight);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while app.tabs[0].as_request().unwrap().in_flight && std::time::Instant::now() < deadline {
            app.drain_messages();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let tab = app.tabs[0].as_request().unwrap();
        assert!(!tab.in_flight, "unresolved URL stayed in_flight");
        assert!(tab.last_error.is_some(), "expected invalid URL error");
    }
}
