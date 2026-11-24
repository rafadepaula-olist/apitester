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
use apitester_import_export::{generate_code, CodegenLanguage};
use apitester_net::{introspect_schema, HttpClient, HttpRequestOptions};
use apitester_proxy::{CaptureProxyServer, CapturedExchange};
use apitester_runner::{
    CollectionRunReport, LoadTestSummary, LoadTimelinePoint, RequestRunResult, RunnerProgressEvent,
};
use apitester_scripting::{run_script, ScriptExecutionInput};
use chrono::Utc;
use egui::{
    CentralPanel, Color32, ComboBox, RichText, ScrollArea, SidePanel, TopBottomPanel, Ui,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::demo::populate_demo_data;
use crate::screenshot_tour::ScreenshotTour;
use crate::state::{
    ImportFormat, LoadProfileKind, ModalState, RequestSubTab, ResponseBodyView, ResponseSubTab,
    RunnerModalTab, SidebarTab, UiMessage, WorkbenchTab,
};
use crate::theme::{apply_theme, highlight_body, method_color, status_color};
use crate::widgets::auth_editor::auth_editor_ui;
use crate::widgets::kv_table::{form_data_table_ui, kv_table_ui, variables_table_ui};
use crate::widgets::modals::render_modals;
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

    pub tabs: Vec<WorkbenchTab>,
    pub active_tab: usize,

    pub sidebar_open: bool,
    pub sidebar_tab: SidebarTab,
    pub search_query: String,
    pub reveal_secrets: bool,
    pub console_open: bool,
    pub console_logs: Vec<ConsoleLogEntry>,

    pub modal: ModalState,

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
    theme_applied: Option<ThemeMode>,
}

impl ApiTesterApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: Handle,
        config: LaunchConfig,
    ) -> Self {
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
            tabs: vec![WorkbenchTab::new_protocol(ProtocolConfig::Http)],
            active_tab: 0,
            sidebar_open: true,
            sidebar_tab: SidebarTab::Collections,
            search_query: String::new(),
            reveal_secrets: false,
            console_open: false,
            console_logs: Vec::new(),
            modal: ModalState::None,
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
        if let Some(idx) = self.tabs.iter().position(|t| t.request.id == req.id) {
            self.active_tab = idx;
            return;
        }
        self.tabs.push(WorkbenchTab::from_request(req, collection_id));
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.tabs.push(WorkbenchTab::new_protocol(ProtocolConfig::Http));
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

    fn drain_messages(&mut self) {
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

                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.in_flight = false;
                        tab.cancel_handle = None;
                        match result {
                            Ok(mut resp) => {
                                resp.console_logs.extend(extra_logs);
                                self.console_logs.extend(resp.console_logs.clone());
                                for cookie in &resp.cookies {
                                    let _ = self.sqlite_store.upsert_cookie(cookie);
                                }
                                if !resp.cookies.is_empty() {
                                    self.cookies = self.sqlite_store.list_cookies().unwrap_or_default();
                                }
                                let entry = HistoryEntry {
                                    id: Uuid::new_v4(),
                                    timestamp: Utc::now(),
                                    method: tab.request.method.clone(),
                                    url: tab.request.url.clone(),
                                    status: Some(resp.status),
                                    duration_ms: Some(resp.timings.total_ms),
                                    request: tab.request.clone(),
                                    response: Some(resp.clone()),
                                };
                                let _ = self.sqlite_store.insert_history(&entry);
                                self.history.insert(0, entry);
                                tab.last_error = None;
                                tab.response = Some(resp);
                            }
                            Err(err) => {
                                tab.last_error = Some(err);
                            }
                        }
                    }
                }
                UiMessage::GraphQlIntrospected { tab_id, result } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        match result {
                            Ok(schema) => tab.graphql_schema_json = Some(schema),
                            Err(err) => tab.last_error = Some(err),
                        }
                    }
                }
                UiMessage::WsConnected { tab_id, client } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.ws_client = Some(client);
                        tab.ws_connected = true;
                    }
                }
                UiMessage::WsLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.ws_log.push(entry);
                        if disconnected {
                            tab.ws_connected = false;
                            tab.ws_client = None;
                        }
                    }
                }
                UiMessage::GrpcFinished { tab_id, result } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.grpc_response = Some(match result {
                            Ok(s) => s,
                            Err(e) => format!("Error: {e}"),
                        });
                    }
                }
                UiMessage::MqttConnected { tab_id, client } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.mqtt_client = Some(client);
                        tab.mqtt_connected = true;
                    }
                }
                UiMessage::MqttLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.mqtt_log.push(entry);
                        if disconnected {
                            tab.mqtt_connected = false;
                            tab.mqtt_client = None;
                        }
                    }
                }
                UiMessage::SocketIoConnected { tab_id, client } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                        tab.sio_client = Some(client);
                        tab.sio_connected = true;
                    }
                }
                UiMessage::SocketIoLog {
                    tab_id,
                    entry,
                    disconnected,
                } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
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
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
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

    fn send_active_http_request(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        let req = tab.request.clone();
        let col_id = tab.collection_id;
        let parent_auth = col_id
            .and_then(|cid| self.collections.iter().find(|c| c.id == cid))
            .map(|c| c.auth.clone())
            .unwrap_or(AuthConfig::NoAuth);

        let mut vars = self.build_variable_context(col_id);
        let settings = self.settings.clone();
        let cookies = self.cookies.clone();
        let tx = self.tx.clone();

        let handle = self.rt.spawn(async move {
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

            // 2. Resolve auth inheritance + apply auth
            let resolver = VariableResolver::new(vars.clone());
            let effective_auth = resolve_auth(&[&req.auth, &parent_auth]);
            let auth_ctx = AuthRequestContext {
                method: req.method.as_str().to_string(),
                url: resolver.resolve(&req.url),
                headers: req
                    .headers
                    .iter()
                    .filter(|h| h.enabled)
                    .map(|h| (resolver.resolve(&h.key), resolver.resolve(&h.value)))
                    .collect(),
                body: match &req.body {
                    RequestBody::Raw { content, .. } => resolver.resolve(content).into_bytes(),
                    _ => Vec::new(),
                },
            };

            let auth_app = match apply_auth(effective_auth, &auth_ctx) {
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

        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.in_flight = true;
            tab.last_error = None;
            tab.cancel_handle = Some(handle.abort_handle());
        }
    }

    fn save_active_tab(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
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
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.collection_id = Some(cid);
            }
        }
    }

    fn render_top_bar(&mut self, ctx: &egui::Context) {
        TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(if self.sidebar_open { "◧" } else { "☰" })
                    .on_hover_text("Toggle Sidebar")
                    .clicked()
                {
                    self.sidebar_open = !self.sidebar_open;
                }
                ui.label(
                    RichText::new("APITester")
                        .strong()
                        .size(16.0)
                        .color(Color32::from_rgb(255, 108, 55)),
                );

                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Search collections & history...")
                        .desired_width(210.0),
                );

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
                            self.tabs.push(WorkbenchTab::new_protocol(proto));
                            self.active_tab = self.tabs.len() - 1;
                            ui.close_menu();
                        }
                    }
                });

                if ui.button("Import/Export").clicked() {
                    self.modal = ModalState::ImportExport {
                        format: ImportFormat::PostmanCollection,
                        input_text: String::new(),
                        status: String::new(),
                    };
                }
                if ui.button("Runner").clicked() {
                    self.modal = ModalState::RunnerAndLoadTester;
                }

                let proxy_label = if self.proxy_server.is_some() {
                    RichText::new("● Proxy ON").color(Color32::from_rgb(52, 211, 153))
                } else {
                    RichText::new("Proxy")
                };
                if ui.button(proxy_label).clicked() {
                    self.modal = ModalState::CaptureProxy;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Settings").clicked() {
                        self.modal = ModalState::Settings;
                    }
                    let theme_icon = match self.settings.theme {
                        ThemeMode::Light => "🌙",
                        _ => "☀",
                    };
                    if ui
                        .button(theme_icon)
                        .on_hover_text("Toggle Dark/Light Theme")
                        .clicked()
                    {
                        self.toggle_theme();
                        apply_theme(ctx, self.settings.theme);
                        self.theme_applied = Some(self.settings.theme);
                    }

                    let active_env_name = self
                        .settings
                        .active_environment_id
                        .and_then(|id| self.environments.iter().find(|e| e.id == id))
                        .map(|e| e.name.as_str())
                        .unwrap_or("No Environment");

                    ComboBox::from_id_salt("active_env_combo")
                        .width(150.0)
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
                                let selected = self.settings.active_environment_id == Some(env.id);
                                if ui.selectable_label(selected, &env.name).clicked() {
                                    self.settings.active_environment_id = Some(env.id);
                                    let _ = self.sqlite_store.save_settings(&self.settings);
                                }
                            }
                        });
                });
            });
            ui.add_space(4.0);
        });
    }

    fn render_sidebar(&mut self, ctx: &egui::Context) {
        if !self.sidebar_open {
            return;
        }
        SidePanel::left("sidebar")
            .default_width(275.0)
            .min_width(210.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.sidebar_tab,
                        SidebarTab::Collections,
                        "Collections",
                    );
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Environments, "Envs");
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::History, "History");
                    ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Cookies, "Cookies");
                });
                ui.separator();

                match self.sidebar_tab {
                    SidebarTab::Collections => self.render_collections_sidebar(ui),
                    SidebarTab::Environments => self.render_environments_sidebar(ui),
                    SidebarTab::History => self.render_history_sidebar(ui),
                    SidebarTab::Cookies => self.render_cookies_sidebar(ui),
                }
            });
    }

    fn render_collections_sidebar(&mut self, ui: &mut Ui) {
        if ui.button("+ New Collection").clicked() {
            let col = Collection::new(format!("Collection {}", self.collections.len() + 1));
            let _ = self.json_store.save_collection(&col);
            self.collections.push(col);
        }
        ui.separator();

        let query = self.search_query.to_ascii_lowercase();
        let mut open_req: Option<(RequestItem, Option<Uuid>)> = None;
        let mut delete_col_id: Option<Uuid> = None;
        let mut edit_col_id: Option<Uuid> = None;
        let mut add_req_col: Option<Uuid> = None;
        let mut add_folder_col: Option<Uuid> = None;

        ScrollArea::vertical().show(ui, |ui| {
            for col in &self.collections {
                egui::CollapsingHeader::new(RichText::new(&col.name).strong())
                    .id_salt(col.id)
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("+ Req").clicked() {
                                add_req_col = Some(col.id);
                            }
                            if ui.small_button("+ Folder").clicked() {
                                add_folder_col = Some(col.id);
                            }
                            if ui.small_button("Auth/Vars").clicked() {
                                edit_col_id = Some(col.id);
                            }
                            if ui.small_button("Del").clicked() {
                                delete_col_id = Some(col.id);
                            }
                        });
                        render_collection_nodes(ui, &col.items, col.id, &query, &mut open_req);
                    });
            }
        });

        if let Some((req, cid)) = open_req {
            self.open_or_focus_request(req, cid);
        }
        if let Some(cid) = edit_col_id {
            self.modal = ModalState::CollectionEditor { collection_id: cid };
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
        ui.horizontal(|ui| {
            if ui.button("+ New Env").clicked() {
                let env = Environment::new(format!("Environment {}", self.environments.len() + 1));
                let _ = self.json_store.save_environment(&env);
                self.environments.push(env);
            }
            ui.checkbox(&mut self.reveal_secrets, "Show secrets");
        });
        ui.separator();

        ScrollArea::vertical().show(ui, |ui| {
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
                    format!("● {}", env.name)
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

    fn render_history_sidebar(&mut self, ui: &mut Ui) {
        if ui.small_button("Clear History").clicked() {
            let _ = self.sqlite_store.clear_history();
            self.history.clear();
        }
        ui.separator();

        let query = self.search_query.to_ascii_lowercase();
        let mut open_entry = None;
        ScrollArea::vertical().show(ui, |ui| {
            for (idx, entry) in self.history.iter().enumerate() {
                if !query.is_empty() && !entry.url.to_ascii_lowercase().contains(&query) {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(entry.method.as_str())
                            .strong()
                            .color(method_color(&entry.method)),
                    );
                    if ui.selectable_label(false, &entry.url).clicked() {
                        open_entry = Some(idx);
                    }
                });
                ui.horizontal(|ui| {
                    if let Some(status) = entry.status {
                        ui.label(
                            RichText::new(format!("{status}"))
                                .small()
                                .color(status_color(status)),
                        );
                    }
                    ui.weak(entry.timestamp.format("%H:%M:%S").to_string());
                });
                ui.separator();
            }
        });

        if let Some(idx) = open_entry {
            let entry = self.history[idx].clone();
            let mut tab = WorkbenchTab::from_request(entry.request, None);
            tab.response = entry.response;
            self.tabs.push(tab);
            self.active_tab = self.tabs.len() - 1;
        }
    }

    fn render_cookies_sidebar(&mut self, ui: &mut Ui) {
        if ui.small_button("Clear All Cookies").clicked() {
            let _ = self.sqlite_store.clear_cookies();
            self.cookies.clear();
        }
        ui.separator();

        let mut del_cookie = None;
        ScrollArea::vertical().show(ui, |ui| {
            for (idx, c) in self.cookies.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.strong(&c.domain);
                    ui.label(format!("{}={}", c.name, c.value));
                    if ui.small_button("×").clicked() {
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

    fn render_console_drawer(&mut self, ctx: &egui::Context) {
        TopBottomPanel::bottom("console_drawer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let arrow = if self.console_open { "▼" } else { "▲" };
                if ui
                    .small_button(format!("{arrow} Wire Console ({})", self.console_logs.len()))
                    .clicked()
                {
                    self.console_open = !self.console_open;
                }
                if self.console_open && ui.small_button("Clear").clicked() {
                    self.console_logs.clear();
                }
            });

            if self.console_open {
                ui.separator();
                ScrollArea::vertical()
                    .max_height(140.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in &self.console_logs {
                            let color = match entry.level {
                                ConsoleLogLevel::Error => Color32::from_rgb(248, 113, 113),
                                ConsoleLogLevel::Warn => Color32::from_rgb(251, 191, 36),
                                ConsoleLogLevel::Info => Color32::from_rgb(96, 165, 250),
                                ConsoleLogLevel::Log => Color32::from_rgb(203, 213, 225),
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.weak(entry.timestamp.format("%H:%M:%S").to_string());
                                ui.label(RichText::new(&entry.message).monospace().color(color));
                            });
                        }
                    });
            }
        });
    }

    fn render_workbench(&mut self, ctx: &egui::Context) {
        CentralPanel::default().show(ctx, |ui| {
            // Tab Bar
            let mut close_idx = None;
            ui.horizontal(|ui| {
                for (idx, tab) in self.tabs.iter().enumerate() {
                    let selected = idx == self.active_tab;
                    let badge = match &tab.request.protocol {
                        ProtocolConfig::Http => tab.request.method.as_str(),
                        ProtocolConfig::GraphQL { .. } => "GQL",
                        ProtocolConfig::WebSocket { .. } => "WS",
                        ProtocolConfig::Grpc { .. } => "gRPC",
                        ProtocolConfig::Mqtt { .. } => "MQTT",
                        ProtocolConfig::SocketIo { .. } => "SIO",
                        ProtocolConfig::Mcp { .. } => "MCP",
                        ProtocolConfig::Sse { .. } => "SSE",
                    };
                    let dirty_mark = if tab.dirty { " •" } else { "" };
                    let label = format!("{badge} {}{dirty_mark}", tab.title);
                    if ui.selectable_label(selected, label).clicked() {
                        self.active_tab = idx;
                    }
                    if ui.small_button("×").clicked() {
                        close_idx = Some(idx);
                    }
                    ui.separator();
                }
                if ui.small_button("+").on_hover_text("New HTTP Tab").clicked() {
                    self.tabs.push(WorkbenchTab::new_protocol(ProtocolConfig::Http));
                    self.active_tab = self.tabs.len() - 1;
                }
            });
            if let Some(idx) = close_idx {
                self.close_tab(idx);
            }
            ui.separator();

            if self.tabs.is_empty() {
                return;
            }

            let col_id = self.tabs[self.active_tab].collection_id;
            let vars = self.build_variable_context(col_id);
            let is_http_like = matches!(
                self.tabs[self.active_tab].request.protocol,
                ProtocolConfig::Http | ProtocolConfig::GraphQL { .. }
            );

            if !is_http_like {
                let rt = self.rt.clone();
                let tx = self.tx.clone();
                let tab = &mut self.tabs[self.active_tab];
                protocol_workbench_ui(ui, tab, &rt, &tx, &vars);
                return;
            }

            self.render_http_workbench(ui, &vars);
        });
    }

    fn render_http_workbench(&mut self, ui: &mut Ui, vars: &VariableContext) {
        let mut trigger_send = false;
        let mut trigger_save = false;
        let mut trigger_code = false;
        let mut trigger_introspect = false;
        let is_dark = !matches!(self.settings.theme, ThemeMode::Light);

        {
            let tab = &mut self.tabs[self.active_tab];
            let resolver = VariableResolver::new(vars.clone());

            // URL & Send bar
            ui.horizontal(|ui| {
                let current_method = tab.request.method.clone();
                ComboBox::from_id_salt("http_method_combo")
                    .width(92.0)
                    .selected_text(
                        RichText::new(current_method.as_str())
                            .strong()
                            .color(method_color(&current_method)),
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
                            let label = RichText::new(m.as_str()).strong().color(method_color(&m));
                            if ui
                                .selectable_value(&mut tab.request.method, m, label)
                                .changed()
                            {
                                tab.dirty = true;
                            }
                        }
                    });

                let url_resp = ui.add(
                    egui::TextEdit::singleline(&mut tab.request.url)
                        .hint_text("https://api.example.com/v1/resource")
                        .desired_width((ui.available_width() - 250.0).max(160.0)),
                );
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

                if tab.in_flight {
                    if ui
                        .button(RichText::new("Cancel").color(Color32::from_rgb(248, 113, 113)))
                        .clicked()
                    {
                        if let Some(h) = tab.cancel_handle.take() {
                            h.abort();
                        }
                        tab.in_flight = false;
                    }
                } else if ui
                    .button(
                        RichText::new("Send")
                            .strong()
                            .color(Color32::from_rgb(255, 108, 55)),
                    )
                    .clicked()
                {
                    trigger_send = true;
                }

                if ui.button("Save").clicked() {
                    trigger_save = true;
                }
                if ui.button("Code").clicked() {
                    trigger_code = true;
                }
            });

            ui.add_space(4.0);

            // Request Sub-tabs
            ui.horizontal(|ui| {
                ui.selectable_value(&mut tab.req_sub_tab, RequestSubTab::Params, "Params");
                ui.selectable_value(&mut tab.req_sub_tab, RequestSubTab::Auth, "Authorization");
                ui.selectable_value(
                    &mut tab.req_sub_tab,
                    RequestSubTab::Headers,
                    format!("Headers ({})", tab.request.headers.len()),
                );
                ui.selectable_value(&mut tab.req_sub_tab, RequestSubTab::Body, "Body");
                ui.selectable_value(
                    &mut tab.req_sub_tab,
                    RequestSubTab::PreRequest,
                    "Pre-request",
                );
                ui.selectable_value(&mut tab.req_sub_tab, RequestSubTab::Tests, "Tests");
                ui.selectable_value(&mut tab.req_sub_tab, RequestSubTab::Settings, "Settings");
            });
            ui.separator();

            // Request Editor Pane (upper half)
            ScrollArea::vertical()
                .id_salt("req_editor_scroll")
                .max_height(230.0)
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
                        render_body_editor(ui, tab, &mut trigger_introspect);
                    }
                    RequestSubTab::PreRequest => {
                        ui.weak("JavaScript executed before the request is sent (pm.* API):");
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
                        ui.weak("JavaScript executed after the response arrives (pm.test / pm.expect):");
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
                            if ui.text_edit_singleline(&mut tab.request.name).changed() {
                                tab.dirty = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Description:");
                            if ui.text_edit_singleline(&mut tab.request.description).changed() {
                                tab.dirty = true;
                            }
                        });
                    }
                });

            ui.separator();

            // Response Pane (lower half)
            if let Some(err) = &tab.last_error {
                ui.colored_label(Color32::from_rgb(248, 113, 113), format!("Error: {err}"));
            }

            if let Some(resp) = tab.response.clone() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} {}", resp.status, resp.status_text))
                            .strong()
                            .color(status_color(resp.status)),
                    );
                    ui.label(format!("{:.1} ms", resp.timings.total_ms)).on_hover_text(format!(
                        "DNS: {:.1} ms\nTCP: {:.1} ms\nTLS: {:.1} ms\nTTFB: {:.1} ms\nTotal: {:.1} ms",
                        resp.timings.dns_ms,
                        resp.timings.tcp_ms,
                        resp.timings.tls_ms,
                        resp.timings.ttfb_ms,
                        resp.timings.total_ms
                    ));
                    ui.label(format_bytes(resp.size_bytes));

                    ui.separator();
                    ui.selectable_value(&mut tab.resp_sub_tab, ResponseSubTab::Body, "Body");
                    ui.selectable_value(
                        &mut tab.resp_sub_tab,
                        ResponseSubTab::Headers,
                        format!("Headers ({})", resp.headers.len()),
                    );
                    ui.selectable_value(
                        &mut tab.resp_sub_tab,
                        ResponseSubTab::Cookies,
                        format!("Cookies ({})", resp.cookies.len()),
                    );
                    let passed = resp.test_results.iter().filter(|t| t.passed).count();
                    ui.selectable_value(
                        &mut tab.resp_sub_tab,
                        ResponseSubTab::TestResults,
                        format!("Tests ({passed}/{})", resp.test_results.len()),
                    );
                    ui.selectable_value(
                        &mut tab.resp_sub_tab,
                        ResponseSubTab::Console,
                        format!("Console ({})", resp.console_logs.len()),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Save Example").clicked() {
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
                });
                ui.separator();

                match tab.resp_sub_tab {
                    ResponseSubTab::Body => {
                        let raw_text = resp.body_text().into_owned();
                        let display_text = match tab.resp_body_view {
                            ResponseBodyView::Pretty => pretty_format_body(&raw_text),
                            ResponseBodyView::Raw => raw_text,
                        };
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut tab.resp_body_view,
                                ResponseBodyView::Pretty,
                                "Pretty",
                            );
                            ui.selectable_value(
                                &mut tab.resp_body_view,
                                ResponseBodyView::Raw,
                                "Raw",
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut tab.resp_search)
                                    .hint_text("Search response...")
                                    .desired_width(180.0),
                            );
                            if ui.small_button("Copy").clicked() {
                                ui.output_mut(|o| o.copied_text = display_text.clone());
                            }
                        });
                        ScrollArea::both().id_salt("resp_body_scroll").show(ui, |ui| {
                            let job = highlight_body(&display_text, is_dark, &tab.resp_search);
                            ui.label(job);
                        });
                    }
                    ResponseSubTab::Headers => {
                        ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("resp_headers_grid")
                                .num_columns(2)
                                .striped(true)
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
                                    ui.weak(format!("{} {}", c.domain, c.path));
                                });
                            }
                        });
                    }
                    ResponseSubTab::TestResults => {
                        ScrollArea::vertical().show(ui, |ui| {
                            for t in &resp.test_results {
                                let (badge, color) = if t.passed {
                                    ("PASS", Color32::from_rgb(52, 211, 153))
                                } else {
                                    ("FAIL", Color32::from_rgb(248, 113, 113))
                                };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(badge).strong().color(color));
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
                    ui.weak("Click Send to execute this request.");
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
            let tab = &self.tabs[self.active_tab];
            let code = generate_code(&tab.request, CodegenLanguage::Curl, vars);
            self.modal = ModalState::CodeSnippet {
                language: CodegenLanguage::Curl,
                code,
            };
        }
        if trigger_introspect {
            let tab = &self.tabs[self.active_tab];
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

impl eframe::App for ApiTesterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ScreenshotTour::tick(self, ctx);

        if self.theme_applied != Some(self.settings.theme) {
            apply_theme(ctx, self.settings.theme);
            self.theme_applied = Some(self.settings.theme);
        }

        self.drain_messages();
        self.render_top_bar(ctx);
        self.render_sidebar(ctx);
        self.render_console_drawer(ctx);
        self.render_workbench(ctx);
        render_modals(ctx, self);

        // Keep repainting smoothly while async requests or realtime streams are active.
        if self.tabs.iter().any(|t| {
            t.in_flight || t.ws_connected || t.mqtt_connected || t.sio_connected || t.sse_connected
        }) || self.proxy_server.is_some()
            || self.runner_in_flight
            || self.load_in_flight
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn render_body_editor(ui: &mut Ui, tab: &mut WorkbenchTab, trigger_introspect: &mut bool) {
    let mode_idx = match tab.request.body {
        RequestBody::None => 0,
        RequestBody::Raw { .. } => 1,
        RequestBody::UrlEncoded(_) => 2,
        RequestBody::FormData(_) => 3,
        RequestBody::Binary { .. } => 4,
        RequestBody::GraphQL { .. } => 5,
    };
    let mut selected = mode_idx;

    ui.horizontal(|ui| {
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
            ui.selectable_value(&mut selected, idx, *label);
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

    ui.add_space(4.0);
    match &mut tab.request.body {
        RequestBody::None => {
            ui.weak("This request does not have a body.");
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
    open_req: &mut Option<(RequestItem, Option<Uuid>)>,
) {
    for node in nodes {
        match node {
            CollectionNode::Folder(folder) => {
                egui::CollapsingHeader::new(&folder.name)
                    .id_salt(folder.id)
                    .default_open(true)
                    .show(ui, |ui| {
                        render_collection_nodes(ui, &folder.items, col_id, query, open_req);
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
                    ui.label(
                        RichText::new(req.method.as_str())
                            .small()
                            .strong()
                            .color(method_color(&req.method)),
                    );
                    if ui.selectable_label(false, &req.name).clicked() {
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
        assert!(app.tabs[0].response.is_some());
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
}
