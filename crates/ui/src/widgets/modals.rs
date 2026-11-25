use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use apitester_core::models::{AuthConfig, ProxyType, ThemeMode};
use apitester_import_export::{
    export_curl, export_postman_collection, export_postman_environment, generate_code, import_curl,
    import_har, import_openapi, import_postman_collection, import_postman_environment,
    CodegenLanguage,
};
use apitester_proxy::{CaptureProxyServer, CertificateAuthority};
use apitester_runner::{
    parse_csv_data, parse_json_data, run_collection, run_load_test, LoadProfile, LoadTestConfig,
    LoadTimelinePoint, RunnerOptions,
};
use egui::{Color32, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2, Window};

use crate::app::ApiTesterApp;
use crate::state::{
    ImportFormat, LoadProfileKind, ModalState, RunnerModalTab, UiMessage, WorkbenchTab,
};
use crate::widgets::auth_editor::auth_editor_ui;
use crate::widgets::kv_table::variables_table_ui;

const CODEGEN_LANGS: &[(CodegenLanguage, &str)] = &[
    (CodegenLanguage::Curl, "cURL"),
    (CodegenLanguage::RustReqwest, "Rust (reqwest)"),
    (CodegenLanguage::PythonRequests, "Python (requests)"),
    (CodegenLanguage::JavaScriptFetch, "JavaScript (fetch)"),
    (CodegenLanguage::GoNetHttp, "Go (net/http)"),
    (CodegenLanguage::PhpCurl, "PHP (cURL)"),
];

/// Renders whichever modal window is currently active.
pub fn render_modals(ctx: &egui::Context, app: &mut ApiTesterApp) {
    let max_h = (ctx.screen_rect().height() - 64.0).max(260.0);
    let max_w = (ctx.screen_rect().width() - 64.0).max(360.0);

    let modal = app.modal.clone();
    match modal {
        ModalState::None => {}
        ModalState::ImportExport {
            mut format,
            mut input_text,
            mut status,
        } => {
            // Handle drag-and-drop files while the Import/Export modal is open.
            let dropped_paths: Vec<_> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect()
            });
            if let Some(path) = dropped_paths.first() {
                let (detected, text, msg) = import_from_file_path(app, path);
                format = detected;
                input_text = text;
                status = msg;
            }

            let mut open = true;
            Window::new("Import / Export (Postman v2.1, cURL, OpenAPI, HAR)")
                .open(&mut open)
                .collapsible(false)
                .default_size([700.0_f32.min(max_w), 500.0_f32.min(max_h)])
                .max_width(max_w)
                .max_height(max_h)
                .vscroll(true)
                .show(ctx, |ui| {
                    // Primary action row always visible at top without scrolling.
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Choose File...")
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(crate::theme::ACCENT_PRIMARY),
                            )
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("API Files", &["json", "yaml", "yml", "har", "txt"])
                                .pick_file()
                            {
                                let (detected, text, msg) = import_from_file_path(app, &path);
                                format = detected;
                                input_text = text;
                                status = msg;
                            }
                        }
                        if ui
                            .add_enabled(
                                !input_text.trim().is_empty(),
                                egui::Button::new(
                                    RichText::new("Import")
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(crate::theme::ACCENT_PRIMARY),
                            )
                            .clicked()
                        {
                            status = run_import(app, format, &input_text);
                        }
                        if ui
                            .add_enabled(!input_text.is_empty(), egui::Button::new("Clear"))
                            .clicked()
                        {
                            input_text.clear();
                            status.clear();
                        }
                        ui.weak("or drag & drop a file here");
                    });

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Format:");
                        ui.selectable_value(
                            &mut format,
                            ImportFormat::PostmanCollection,
                            "Postman v2.1",
                        );
                        ui.selectable_value(
                            &mut format,
                            ImportFormat::PostmanEnvironment,
                            "Postman Env",
                        );
                        ui.selectable_value(&mut format, ImportFormat::Curl, "cURL");
                        ui.selectable_value(&mut format, ImportFormat::OpenApi, "OpenAPI / Swagger");
                        ui.selectable_value(&mut format, ImportFormat::Har, "HAR 1.2");
                    });

                    ui.add_space(4.0);
                    ScrollArea::vertical()
                        .id_salt("import_export_text_scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut input_text)
                                    .desired_rows(12)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("Paste Postman v2.1 JSON, cURL command, OpenAPI JSON/YAML, or HAR 1.2 JSON...")
                                    .code_editor(),
                            );
                        });

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Export Postman v2.1").clicked() {
                            if let Some(col) = app.collections.first() {
                                match export_postman_collection(col) {
                                    Ok(json) => {
                                        input_text = json;
                                        status = format!("Exported '{}' as Postman v2.1 JSON.", col.name);
                                    }
                                    Err(e) => status = format!("Export failed: {e}"),
                                }
                            } else {
                                status = "No collections to export.".into();
                            }
                        }
                        if ui.button("Export Active Env").clicked() {
                            if let Some(env) = app
                                .settings
                                .active_environment_id
                                .and_then(|id| app.environments.iter().find(|e| e.id == id))
                                .or_else(|| app.environments.first())
                            {
                                match export_postman_environment(env) {
                                    Ok(json) => {
                                        input_text = json;
                                        status = format!("Exported environment '{}'.", env.name);
                                    }
                                    Err(e) => status = format!("Export failed: {e}"),
                                }
                            } else {
                                status = "No environments to export.".into();
                            }
                        }
                        if ui.button("Export Active Tab as cURL").clicked() {
                            if let Some(tab) = app.tabs.get(app.active_tab) {
                                let vars = app.build_variable_context(tab.collection_id);
                                input_text = export_curl(&tab.request, &vars);
                                status = "Exported active request as cURL.".into();
                            }
                        }
                        if ui
                            .add_enabled(!input_text.trim().is_empty(), egui::Button::new("Save to File..."))
                            .clicked()
                        {
                            let default_name = match format {
                                ImportFormat::PostmanEnvironment => "environment.postman_environment.json",
                                ImportFormat::Curl => "request.sh",
                                _ => "collection.postman_collection.json",
                            };
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(default_name)
                                .save_file()
                            {
                                status = match std::fs::write(&path, &input_text) {
                                    Ok(()) => format!("Saved to {}", path.display()),
                                    Err(e) => format!("Save failed: {e}"),
                                };
                            }
                        }
                    });

                    if !status.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&status).color(Color32::from_rgb(96, 165, 250)));
                    }
                });
            app.modal = if open {
                ModalState::ImportExport {
                    format,
                    input_text,
                    status,
                }
            } else {
                ModalState::None
            };
        }
        ModalState::CodeSnippet {
            mut language,
            mut code,
        } => {
            let mut open = true;
            Window::new("Code Snippet Generator")
                .open(&mut open)
                .collapsible(false)
                .default_size([660.0_f32.min(max_w), 440.0_f32.min(max_h)])
                .max_width(max_w)
                .max_height(max_h)
                .vscroll(true)
                .show(ctx, |ui| {
                    let prev = language;
                    ui.horizontal_wrapped(|ui| {
                        for &(lang, name) in CODEGEN_LANGS {
                            ui.selectable_value(&mut language, lang, name);
                        }
                        if ui.button("Copy").clicked() {
                            ui.output_mut(|o| o.copied_text = code.clone());
                        }
                    });
                    if prev != language {
                        if let Some(tab) = app.tabs.get(app.active_tab) {
                            let vars = app.build_variable_context(tab.collection_id);
                            code = generate_code(&tab.request, language, &vars);
                        }
                    }
                    ui.separator();
                    ScrollArea::vertical()
                        .id_salt("codegen_snippet_scroll")
                        .max_height(300.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut code)
                                    .desired_rows(16)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            );
                        });
                });
            app.modal = if open {
                ModalState::CodeSnippet { language, code }
            } else {
                ModalState::None
            };
        }
        ModalState::CaptureProxy => {
            let mut open = true;
            let mut close_now = false;
            Window::new("Local Capture Proxy")
                .open(&mut open)
                .collapsible(false)
                .default_size([740.0_f32.min(max_w), 460.0_f32.min(max_h)])
                .max_width(max_w)
                .max_height(max_h)
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Port:");
                        ui.add(egui::DragValue::new(&mut app.proxy_port).range(1024..=65535));
                        if app.proxy_server.is_some() {
                            if ui.button("Stop Proxy").clicked() {
                                if let Some(srv) = app.proxy_server.take() {
                                    srv.stop();
                                }
                                app.proxy_status = "Stopped".into();
                            }
                            ui.label(
                                RichText::new(&app.proxy_status)
                                    .color(Color32::from_rgb(52, 211, 153)),
                            );
                        } else if ui.button("Start Proxy").clicked() {
                            let addr: SocketAddr = ([127, 0, 0, 1], app.proxy_port).into();
                            let start_res = app.rt.block_on(CaptureProxyServer::start(addr));
                            match start_res {
                                Ok((srv, bound_addr, mut rx)) => {
                                    app.proxy_status = format!("Listening on {bound_addr}");
                                    app.proxy_server = Some(srv);
                                    let tx = app.tx.clone();
                                    app.rt.spawn(async move {
                                        while let Ok(exchange) = rx.recv().await {
                                            let _ = tx.send(UiMessage::ProxyCaptured(exchange));
                                        }
                                    });
                                }
                                Err(e) => {
                                    app.proxy_status = format!("Start failed: {e}");
                                }
                            }
                        }

                        if ui.button("Export Root CA").clicked() {
                            let ca_dir = app.json_store.root_dir().join("ca");
                            let _ = std::fs::create_dir_all(&ca_dir);
                            let cert_path = ca_dir.join("apitester-ca.crt.pem");
                            let key_path = ca_dir.join("apitester-ca.key.pem");
                            match CertificateAuthority::load_or_generate(&cert_path, &key_path) {
                                Ok(_) => {
                                    app.proxy_status =
                                        format!("CA saved to {}", cert_path.display());
                                }
                                Err(e) => {
                                    app.proxy_status = format!("CA error: {e}");
                                }
                            }
                        }

                        if ui.button("Clear").clicked() {
                            app.captured_exchanges.clear();
                            if let Some(srv) = &app.proxy_server {
                                srv.clear_history();
                            }
                        }
                    });

                    ui.separator();
                    let mut open_idx = None;
                    ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("proxy_captured_grid")
                            .num_columns(5)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("Method");
                                ui.strong("Status");
                                ui.strong("URL");
                                ui.strong("Time");
                                ui.strong("Action");
                                ui.end_row();
                                for (idx, ex) in app.captured_exchanges.iter().enumerate() {
                                    ui.strong(&ex.method);
                                    ui.label(
                                        RichText::new(format!("{}", ex.status))
                                            .color(crate::theme::status_color(ex.status)),
                                    );
                                    ui.monospace(&ex.url);
                                    ui.weak(format!("{} ms", ex.duration_ms));
                                    if ui.small_button("Open as Request").clicked() {
                                        open_idx = Some(idx);
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                    if let Some(idx) = open_idx {
                        let req = app.captured_exchanges[idx].to_request_item();
                        app.tabs.push(WorkbenchTab::from_request(req, None));
                        app.active_tab = app.tabs.len() - 1;
                        close_now = true;
                    }
                });
            if !open || close_now {
                app.modal = ModalState::None;
            }
        }
        ModalState::RunnerAndLoadTester => {
            let mut open = true;
            Window::new("Collection Runner & Native Load Tester")
                .open(&mut open)
                .collapsible(false)
                .default_size([760.0_f32.min(max_w), 520.0_f32.min(max_h)])
                .max_width(max_w)
                .max_height(max_h)
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut app.runner_modal_tab,
                            RunnerModalTab::CollectionRunner,
                            "Collection Runner",
                        );
                        ui.selectable_value(
                            &mut app.runner_modal_tab,
                            RunnerModalTab::LoadTester,
                            "Load Tester",
                        );
                    });
                    ui.separator();

                    match app.runner_modal_tab {
                        RunnerModalTab::CollectionRunner => render_collection_runner_tab(ui, app),
                        RunnerModalTab::LoadTester => render_load_tester_tab(ui, app),
                    }
                });
            if !open {
                app.modal = ModalState::None;
            }
        }
        ModalState::Settings => {
            let mut open = true;
            let mut close_now = false;
            Window::new("Settings")
                .open(&mut open)
                .collapsible(false)
                .default_size([520.0_f32.min(max_w), 420.0_f32.min(max_h)])
                .max_width(max_w)
                .max_height(max_h)
                .vscroll(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Theme:");
                        ui.selectable_value(&mut app.settings.theme, ThemeMode::Dark, "Dark");
                        ui.selectable_value(&mut app.settings.theme, ThemeMode::Light, "Light");
                    });
                    ui.checkbox(&mut app.settings.verify_ssl, "Verify SSL certificates");
                    ui.checkbox(&mut app.settings.follow_redirects, "Follow redirects");
                    ui.horizontal(|ui| {
                        ui.label("Max Redirects:");
                        ui.add(egui::DragValue::new(&mut app.settings.max_redirects).range(0..=50));
                        ui.label("Timeout (ms):");
                        ui.add(egui::DragValue::new(&mut app.settings.timeout_ms).range(0..=300_000));
                    });

                    ui.separator();
                    ui.strong("Outbound Proxy");
                    ui.checkbox(&mut app.settings.proxy.enabled, "Enable outbound proxy");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("settings_proxy_type")
                            .selected_text(format!("{:?}", app.settings.proxy.proxy_type))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut app.settings.proxy.proxy_type,
                                    ProxyType::Http,
                                    "HTTP",
                                );
                                ui.selectable_value(
                                    &mut app.settings.proxy.proxy_type,
                                    ProxyType::Https,
                                    "HTTPS",
                                );
                                ui.selectable_value(
                                    &mut app.settings.proxy.proxy_type,
                                    ProxyType::Socks5,
                                    "SOCKS5",
                                );
                            });
                        ui.add(
                            egui::TextEdit::singleline(&mut app.settings.proxy.host)
                                .desired_width(180.0),
                        );
                        ui.add(egui::DragValue::new(&mut app.settings.proxy.port).range(1..=65535));
                    });

                    ui.separator();
                    if ui.button("Save Settings").clicked() {
                        let _ = app.sqlite_store.save_settings(&app.settings);
                        crate::theme::apply_theme(ctx, app.settings.theme);
                        close_now = true;
                    }
                });
            if !open || close_now {
                app.modal = ModalState::None;
            }
        }
        ModalState::CollectionEditor { collection_id } => {
            let mut open = true;
            let mut close_now = false;
            if let Some(col) = app.collections.iter_mut().find(|c| c.id == collection_id) {
                let mut save = false;
                Window::new(format!("Collection Settings — {}", col.name))
                    .open(&mut open)
                    .collapsible(false)
                    .default_size([560.0_f32.min(max_w), 440.0_f32.min(max_h)])
                    .max_width(max_w)
                    .max_height(max_h)
                    .vscroll(true)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut col.name);
                        });
                        ui.separator();
                        ui.strong("Collection Authorization");
                        auth_editor_ui(ui, "col_auth_editor", &mut col.auth);
                        ui.separator();
                        ui.strong("Collection Variables");
                        variables_table_ui(
                            ui,
                            "col_vars_editor",
                            &mut col.variables,
                            app.reveal_secrets,
                        );
                        ui.separator();
                        if ui.button("Save Collection").clicked() {
                            save = true;
                            close_now = true;
                        }
                    });
                if save {
                    if let Some(col) = app.collections.iter().find(|c| c.id == collection_id) {
                        let _ = app.json_store.save_collection(col);
                    }
                }
            } else {
                open = false;
            }
            if !open || close_now {
                app.modal = ModalState::None;
            }
        }
    }
}

/// Infers the most likely [`ImportFormat`] from a file extension and content snippet.
pub fn detect_import_format(path: &Path, content: &str) -> ImportFormat {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if ext == "har" {
        return ImportFormat::Har;
    }
    if ext == "yaml" || ext == "yml" {
        return ImportFormat::OpenApi;
    }

    let trimmed = content.trim_start();
    if trimmed.starts_with("curl ") {
        return ImportFormat::Curl;
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if val.get("openapi").is_some() || val.get("swagger").is_some() {
            return ImportFormat::OpenApi;
        }
        if val.get("log").and_then(|l| l.get("entries")).is_some() {
            return ImportFormat::Har;
        }
        if val.get("_postman_variable_scope").is_some()
            || (val.get("values").and_then(|v| v.as_array()).is_some() && val.get("item").is_none())
        {
            return ImportFormat::PostmanEnvironment;
        }
    }

    ImportFormat::PostmanCollection
}

/// Reads an API file from disk, auto-detects its format, and imports it into `app`.
pub fn import_from_file_path(
    app: &mut ApiTesterApp,
    path: &Path,
) -> (ImportFormat, String, String) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let format = detect_import_format(path, &content);
            let status = run_import(app, format, &content);
            (format, content, status)
        }
        Err(e) => (
            ImportFormat::PostmanCollection,
            String::new(),
            format!("Failed to read {}: {e}", path.display()),
        ),
    }
}

fn run_import(app: &mut ApiTesterApp, format: ImportFormat, input: &str) -> String {
    match format {
        ImportFormat::PostmanCollection => match import_postman_collection(input) {
            Ok(col) => {
                let name = col.name.clone();
                let count = col.items.len();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported Postman v2.1 collection '{name}' ({count} top-level items).")
            }
            Err(e) => format!("Postman import failed: {e}"),
        },
        ImportFormat::PostmanEnvironment => match import_postman_environment(input) {
            Ok(env) => {
                let name = env.name.clone();
                let _ = app.json_store.save_environment(&env);
                app.environments.push(env);
                format!("Imported Postman environment '{name}'.")
            }
            Err(e) => format!("Environment import failed: {e}"),
        },
        ImportFormat::Curl => match import_curl(input) {
            Ok(req) => {
                let url = req.url.clone();
                app.tabs.push(WorkbenchTab::from_request(req, None));
                app.active_tab = app.tabs.len() - 1;
                format!("Imported cURL request for '{url}' into a new tab.")
            }
            Err(e) => format!("cURL import failed: {e}"),
        },
        ImportFormat::OpenApi => match import_openapi(input) {
            Ok(col) => {
                let name = col.name.clone();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported OpenAPI/Swagger spec as collection '{name}'.")
            }
            Err(e) => format!("OpenAPI import failed: {e}"),
        },
        ImportFormat::Har => match import_har(input) {
            Ok(col) => {
                let name = col.name.clone();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported HAR 1.2 recording as collection '{name}'.")
            }
            Err(e) => format!("HAR import failed: {e}"),
        },
    }
}

fn render_collection_runner_tab(ui: &mut egui::Ui, app: &mut ApiTesterApp) {
    ui.horizontal(|ui| {
        ui.label("Iterations:");
        ui.add(egui::DragValue::new(&mut app.runner_iterations).range(1..=1000));
        ui.label("Delay (ms):");
        ui.add(egui::DragValue::new(&mut app.runner_delay_ms).range(0..=60_000));
        ui.checkbox(&mut app.runner_stop_on_error, "Stop on error");
    });

    ui.collapsing("Iteration Data (CSV or JSON array)", |ui| {
        ScrollArea::vertical()
            .id_salt("runner_data_scroll")
            .max_height(140.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut app.runner_data_input)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("id,name\n1,Ada\n2,Grace  -- or JSON [{\"id\":\"1\"}]")
                        .code_editor(),
                );
            });
    });

    ui.add_space(4.0);
    let mut start_col_idx = None;
    ui.horizontal_wrapped(|ui| {
        for (idx, col) in app.collections.iter().enumerate() {
            if ui
                .add_enabled(
                    !app.runner_in_flight,
                    egui::Button::new(format!("Run '{}'", col.name)),
                )
                .clicked()
            {
                start_col_idx = Some(idx);
            }
        }
        if app.runner_in_flight {
            ui.spinner();
            ui.label("Running collection...");
        }
    });

    if let Some(idx) = start_col_idx {
        let col = app.collections[idx].clone();
        let vars = app.build_variable_context(Some(col.id));
        let trimmed = app.runner_data_input.trim();
        let data_rows = if trimmed.is_empty() {
            Vec::new()
        } else if trimmed.starts_with('[') {
            parse_json_data(trimmed).unwrap_or_default()
        } else {
            parse_csv_data(trimmed).unwrap_or_default()
        };

        let opts = RunnerOptions {
            iterations: app.runner_iterations,
            delay_ms: app.runner_delay_ms,
            data_rows,
            stop_on_error: app.runner_stop_on_error,
            script_timeout_ms: 5_000,
            settings: app.settings.clone(),
        };

        app.runner_in_flight = true;
        app.runner_live_results.clear();
        app.runner_report = None;

        let tx = app.tx.clone();
        app.rt.spawn(async move {
            let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
            let tx_prog = tx.clone();
            let fwd = tokio::spawn(async move {
                while let Some(ev) = prx.recv().await {
                    let _ = tx_prog.send(UiMessage::RunnerProgress(ev));
                }
            });
            let report = run_collection(&col, vars, opts, Some(ptx)).await;
            let _ = fwd.await;
            let _ = tx.send(UiMessage::RunnerFinished(report));
        });
    }

    ui.separator();

    if let Some(report) = &app.runner_report {
        ui.horizontal(|ui| {
            ui.strong(format!("Collection: {}", report.collection_name));
            ui.label(format!("Requests: {}", report.total_requests));
            ui.colored_label(
                Color32::from_rgb(52, 211, 153),
                format!("Passed Tests: {}", report.passed_tests),
            );
            ui.colored_label(
                if report.failed_tests > 0 || report.failed_requests > 0 {
                    Color32::from_rgb(248, 113, 113)
                } else {
                    Color32::from_rgb(139, 148, 165)
                },
                format!("Failed Tests: {}", report.failed_tests),
            );
            ui.weak(format!("{:.1} ms", report.total_duration_ms));
        });
        ui.separator();
    }

    ScrollArea::vertical().id_salt("runner_results_scroll").show(ui, |ui| {
        egui::Grid::new("runner_results_grid")
            .num_columns(5)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Iter");
                ui.strong("Request");
                ui.strong("Status");
                ui.strong("Time");
                ui.strong("Tests");
                ui.end_row();
                for r in &app.runner_live_results {
                    ui.label(format!("#{}", r.iteration));
                    ui.label(format!("{} {}", r.method, r.request_name));
                    match r.status {
                        Some(code) => {
                            ui.label(
                                RichText::new(format!("{code}"))
                                    .color(crate::theme::status_color(code)),
                            );
                        }
                        None => {
                            ui.colored_label(
                                Color32::from_rgb(248, 113, 113),
                                r.error.as_deref().unwrap_or("ERR"),
                            );
                        }
                    }
                    ui.weak(format!("{:.1} ms", r.duration_ms));
                    let passed = r.test_results.iter().filter(|t| t.passed).count();
                    ui.label(format!("{passed}/{}", r.test_results.len()));
                    ui.end_row();
                }
            });
    });
}

fn render_load_tester_tab(ui: &mut egui::Ui, app: &mut ApiTesterApp) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Profile:");
        ui.selectable_value(&mut app.load_profile_kind, LoadProfileKind::Constant, "Constant");
        ui.selectable_value(&mut app.load_profile_kind, LoadProfileKind::Ramp, "Ramp");
        ui.selectable_value(&mut app.load_profile_kind, LoadProfileKind::Spike, "Spike");
        ui.selectable_value(&mut app.load_profile_kind, LoadProfileKind::Peak, "Peak");
    });

    ui.horizontal(|ui| {
        ui.label("VUs:");
        ui.add(egui::DragValue::new(&mut app.load_vus).range(1..=500));
        if app.load_profile_kind != LoadProfileKind::Constant {
            ui.label("Peak/End VUs:");
            ui.add(egui::DragValue::new(&mut app.load_peak_vus).range(1..=500));
        }
        ui.label("Duration (s):");
        ui.add(egui::DragValue::new(&mut app.load_duration_secs).range(1..=300));

        if app.load_in_flight {
            if ui
                .button(RichText::new("Stop").color(Color32::from_rgb(248, 113, 113)))
                .clicked()
            {
                if let Some(flag) = &app.load_cancel {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            ui.spinner();
        } else if ui
            .add(
                egui::Button::new(
                    RichText::new("Start Load Test")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(crate::theme::ACCENT_PRIMARY),
            )
            .clicked()
        {
            if let Some(tab) = app.tabs.get(app.active_tab) {
                let req = tab.request.clone();
                let vars = app.build_variable_context(tab.collection_id);
                let auth = tab
                    .collection_id
                    .and_then(|cid| app.collections.iter().find(|c| c.id == cid))
                    .map(|c| c.auth.clone())
                    .unwrap_or(AuthConfig::NoAuth);
                let effective_auth = match &req.auth {
                    AuthConfig::Inherit => auth,
                    other => other.clone(),
                };

                let profile = match app.load_profile_kind {
                    LoadProfileKind::Constant => LoadProfile::Constant { vus: app.load_vus },
                    LoadProfileKind::Ramp => LoadProfile::Ramp {
                        start_vus: app.load_vus,
                        end_vus: app.load_peak_vus,
                    },
                    LoadProfileKind::Spike => LoadProfile::Spike {
                        base_vus: app.load_vus,
                        spike_vus: app.load_peak_vus,
                        spike_at_pct: 0.4,
                        spike_duration_pct: 0.25,
                    },
                    LoadProfileKind::Peak => LoadProfile::Peak {
                        base_vus: app.load_vus,
                        peak_vus: app.load_peak_vus,
                    },
                };

                let cancel = Arc::new(AtomicBool::new(false));
                app.load_cancel = Some(cancel.clone());
                app.load_in_flight = true;
                app.load_timeline.clear();
                app.load_summary = None;

                let cfg = LoadTestConfig {
                    duration_secs: app.load_duration_secs,
                    profile,
                    settings: app.settings.clone(),
                    cancel,
                };
                let tx = app.tx.clone();
                app.rt.spawn(async move {
                    let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
                    let tx_prog = tx.clone();
                    let fwd = tokio::spawn(async move {
                        while let Some(up) = prx.recv().await {
                            let _ = tx_prog.send(UiMessage::LoadProgress(up));
                        }
                    });
                    let summary = run_load_test(req, vars, effective_auth, cfg, Some(ptx)).await;
                    let _ = fwd.await;
                    let _ = tx.send(UiMessage::LoadFinished(summary));
                });
            }
        }
    });

    ui.separator();

    if let Some(summary) = &app.load_summary {
        ui.horizontal_wrapped(|ui| {
            metric_badge(ui, "RPS", &format!("{:.1}", summary.rps), Color32::from_rgb(52, 211, 153));
            metric_badge(
                ui,
                "Requests",
                &format!("{}", summary.total_requests),
                Color32::from_rgb(96, 165, 250),
            );
            metric_badge(
                ui,
                "Error Rate",
                &format!("{:.2}%", summary.error_rate * 100.0),
                if summary.error_count > 0 {
                    Color32::from_rgb(248, 113, 113)
                } else {
                    Color32::from_rgb(52, 211, 153)
                },
            );
            metric_badge(ui, "p50", &format!("{:.1} ms", summary.p50_ms), Color32::from_rgb(203, 213, 225));
            metric_badge(ui, "p90", &format!("{:.1} ms", summary.p90_ms), Color32::from_rgb(251, 191, 36));
            metric_badge(ui, "p95", &format!("{:.1} ms", summary.p95_ms), Color32::from_rgb(251, 146, 60));
            metric_badge(ui, "p99", &format!("{:.1} ms", summary.p99_ms), Color32::from_rgb(248, 113, 113));
        });
        ui.add_space(6.0);
        draw_load_chart(ui, &summary.timeline);
    } else if !app.load_timeline.is_empty() {
        draw_load_chart(ui, &app.load_timeline);
    } else {
        ui.weak("Select a load profile and click Start Load Test to benchmark the active request.");
    }
}

fn metric_badge(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.weak(label);
            ui.label(RichText::new(value).strong().size(15.0).color(color));
        });
    });
}

fn draw_load_chart(ui: &mut egui::Ui, points: &[LoadTimelinePoint]) {
    if points.is_empty() {
        return;
    }
    ui.label(RichText::new("Throughput (RPS bars) & p95 Latency (orange line)").small());
    let desired = Vec2::new(ui.available_width().max(320.0), 180.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(16, 20, 27));
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::from_rgb(46, 53, 66)));

    let pad = 16.0_f32;
    let plot = Rect::from_min_max(
        Pos2::new(rect.min.x + pad, rect.min.y + pad),
        Pos2::new(rect.max.x - pad, rect.max.y - pad),
    );

    let max_rps = points
        .iter()
        .map(|p| p.requests)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let max_p95 = points
        .iter()
        .map(|p| p.p95_latency_ms as f32)
        .fold(1.0_f32, f32::max);

    let n = points.len() as f32;
    let bar_w = (plot.width() / n).max(4.0);
    let mut line_pts = Vec::with_capacity(points.len());

    for (idx, pt) in points.iter().enumerate() {
        let x0 = plot.min.x + idx as f32 * bar_w + 2.0;
        let x1 = (x0 + bar_w - 4.0).max(x0 + 2.0);
        let h_rps = (pt.requests as f32 / max_rps) * plot.height();
        let bar_rect = Rect::from_min_max(
            Pos2::new(x0, plot.max.y - h_rps),
            Pos2::new(x1, plot.max.y),
        );
        painter.rect_filled(bar_rect, 2.0, Color32::from_rgba_unmultiplied(52, 211, 153, 140));

        let cx = (x0 + x1) * 0.5;
        let cy = plot.max.y - (pt.p95_latency_ms as f32 / max_p95) * plot.height();
        line_pts.push(Pos2::new(cx, cy));
    }

    if line_pts.len() >= 2 {
        painter.add(egui::Shape::line(
            line_pts,
            Stroke::new(2.0_f32, Color32::from_rgb(255, 108, 55)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{tempdir, NamedTempFile};

    use super::*;
    use crate::app::LaunchConfig;

    #[test]
    fn import_from_file_path_loads_postman_v21_collection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let data_dir = tempdir().unwrap();
        let mut app = ApiTesterApp::new_headless(
            rt.handle().clone(),
            LaunchConfig {
                demo_mode: false,
                data_dir: Some(data_dir.path().to_path_buf()),
                screenshot_dir: None,
            },
        );
        assert!(app.collections.is_empty());

        let mut tmp = NamedTempFile::new().unwrap();
        let postman_json = r#"{
            "info": {
                "name": "File Imported API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [
                {
                    "name": "Health Check",
                    "request": {
                        "method": "GET",
                        "url": "https://api.example.com/health"
                    }
                }
            ]
        }"#;
        tmp.write_all(postman_json.as_bytes()).unwrap();

        let (format, content, status) = import_from_file_path(&mut app, tmp.path());
        assert_eq!(format, ImportFormat::PostmanCollection);
        assert!(!content.is_empty());
        assert!(status.contains("File Imported API"), "status: {status}");
        assert_eq!(app.collections.len(), 1);
        assert_eq!(app.collections[0].name, "File Imported API");
        assert_eq!(app.collections[0].items.len(), 1);
    }
}
